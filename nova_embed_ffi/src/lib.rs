// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! # The C ABI of `nova_embed`
//!
//! The only crate of the engine with `unsafe`, and only for what a C ABI
//! cannot do without it: reading the caller's (pointer, length) buffers,
//! writing the caller's out-structs, owning the opaque engine handle and
//! result buffers across calls, and calling the two C callbacks. Every
//! buffer is copied into an owned Rust value on entry; nothing the engine
//! owns is lent to C except the argument views of a host function call, which
//! are valid during that call only.
//!
//! The header is `include/nova_engine.h`; its comments are the contract.
//!
//! Exported symbols carry `#[unsafe(no_mangle)]`: the names are unique (all
//! start with `nova_`), so no other symbol of the process is replaced.

#![deny(unsafe_op_in_unsafe_fn)]

use core::{
    cell::{Cell, RefCell},
    ffi::c_void,
    panic::AssertUnwindSafe,
};
use std::{panic::catch_unwind, rc::Rc, sync::LazyLock};

use nova_embed::{
    ArgumentMode, CompiledScript, Completion, Engine, EngineOptions, Error, Host, HostArg,
    HostReturn,
};

pub const NOVA_OK: i32 = 0;
pub const NOVA_EXCEPTION: i32 = 1;
pub const NOVA_NOT_A_FUNCTION: i32 = 2;
pub const NOVA_INTERRUPTED: i32 = 3;
pub const NOVA_OUT_OF_MEMORY: i32 = 4;
pub const NOVA_DEAD: i32 = 5;
pub const NOVA_REFUSED: i32 = 6;
pub const NOVA_INVALID: i32 = 7;

pub const NOVA_VALUE_UNDEFINED: i32 = 0;
pub const NOVA_VALUE_NULL: i32 = 1;
pub const NOVA_VALUE_STRING: i32 = 2;
pub const NOVA_VALUE_NUMBER: i32 = 3;
pub const NOVA_VALUE_BYTES: i32 = 4;
pub const NOVA_VALUE_OTHER: i32 = 5;

pub const NOVA_ARGS_VALUES: i32 = 0;
pub const NOVA_ARGS_VALUES_AND_BYTES: i32 = 1;

/// The most bytes of one input buffer (script source, name, argument).
pub const NOVA_MAX_INPUT: usize = 1 << 30;
/// The most arguments `nova_engine_call` takes.
pub const NOVA_MAX_CALL_ARGS: usize = 64;

/// A value or an error text owned by the library; `nova_result_free` frees
/// `bytes`.
#[repr(C)]
pub struct NovaResult {
    pub kind: i32,
    pub number: f64,
    pub bytes: *mut u8,
    pub len: usize,
}

/// A borrowed byte string.
#[repr(C)]
pub struct NovaStr {
    pub ptr: *const u8,
    pub len: usize,
}

/// A host function argument, valid during the callback only.
#[repr(C)]
pub struct NovaArg {
    pub kind: i32,
    pub array_kind: i32,
    pub number: f64,
    pub bytes: *const u8,
    pub len: usize,
}

/// Where a host function puts its return value (`nova_return_*`); opaque to
/// C.
pub struct NovaReturn {
    value: HostReturn,
}

pub type NovaHostFn =
    Option<unsafe extern "C" fn(*mut c_void, u32, *const NovaArg, usize, *mut NovaReturn)>;
pub type NovaInterruptFn = Option<unsafe extern "C" fn(*mut c_void) -> bool>;
pub type NovaTimeZoneFn = Option<unsafe extern "C" fn(*mut c_void, i64) -> i32>;

/// The opaque engine handle. The `RefCell` refuses a call made from inside
/// one of the engine's own callbacks instead of aliasing the engine; a free
/// requested from there waits for the outermost call to return.
pub struct NovaEngine {
    engine: RefCell<Engine>,
    time_zone: Rc<Cell<NovaTimeZoneFn>>,
    free_requested: Cell<bool>,
}

struct CHost {
    call: unsafe extern "C" fn(*mut c_void, u32, *const NovaArg, usize, *mut NovaReturn),
    interrupt: NovaInterruptFn,
    time_zone: Rc<Cell<NovaTimeZoneFn>>,
    context: *mut c_void,
}

fn arg_view(arg: &HostArg) -> NovaArg {
    let mut view = NovaArg {
        kind: NOVA_VALUE_OTHER,
        array_kind: 0,
        number: 0.0,
        bytes: core::ptr::null(),
        len: 0,
    };
    match arg {
        HostArg::Other => {}
        HostArg::String(text) => {
            view.kind = NOVA_VALUE_STRING;
            view.bytes = text.as_ptr();
            view.len = text.len();
        }
        HostArg::Number(number) => {
            view.kind = NOVA_VALUE_NUMBER;
            view.number = *number;
        }
        HostArg::Bytes { kind, bytes } => {
            view.kind = NOVA_VALUE_BYTES;
            view.array_kind = *kind as i32;
            view.bytes = bytes.as_ptr();
            view.len = bytes.len();
        }
    }
    view
}

impl Host for CHost {
    fn call(&self, id: u32, args: Vec<HostArg>) -> HostReturn {
        let views: Vec<NovaArg> = args.iter().map(arg_view).collect();
        let mut ret = NovaReturn {
            value: HostReturn::Undefined,
        };
        // SAFETY: `call` and `context` are what the embedder passed to
        // `nova_engine_new`, which requires the function to stay callable
        // with that context for the engine's life. `views` and the `args`
        // they point into outlive the call; `ret` is a live, exclusive
        // NovaReturn for the length of the call (the header says it must not
        // be kept).
        unsafe { (self.call)(self.context, id, views.as_ptr(), views.len(), &mut ret) };
        drop(views);
        drop(args);
        ret.value
    }

    fn should_interrupt(&self) -> bool {
        match self.interrupt {
            // SAFETY: as for `call`: the embedder keeps the interrupt function
            // callable with its context for the engine's life.
            Some(interrupt) => unsafe { interrupt(self.context) },
            None => false,
        }
    }

    fn local_time_zone_offset(&self, epoch_seconds: i64) -> i32 {
        match self.time_zone.get() {
            // SAFETY: `nova_engine_set_time_zone` requires the function to
            // stay callable with the engine's context until the engine is
            // freed or the function replaced.
            Some(time_zone) => unsafe { time_zone(self.context, epoch_seconds) },
            None => 0,
        }
    }
}

/// Copies (ptr, len) into an owned buffer.
///
/// # Safety
/// `ptr` must be null with `len` 0, or valid for reads of `len` bytes for the
/// length of the call.
unsafe fn read_bytes(ptr: *const u8, len: usize) -> Option<Vec<u8>> {
    if len == 0 {
        return Some(Vec::new());
    }
    if ptr.is_null() || len > NOVA_MAX_INPUT {
        return None;
    }
    // SAFETY: non-null, at most 1 GiB (so below isize::MAX), and readable for
    // `len` bytes by the caller's contract; copied before returning.
    Some(unsafe { core::slice::from_raw_parts(ptr, len) }.to_vec())
}

/// # Safety
/// As `read_bytes`.
unsafe fn read_utf8(ptr: *const u8, len: usize) -> Option<String> {
    // SAFETY: forwarded contract.
    String::from_utf8(unsafe { read_bytes(ptr, len) }?).ok()
}

fn owned_bytes(bytes: Vec<u8>) -> (*mut u8, usize) {
    if bytes.is_empty() {
        return (core::ptr::null_mut(), 0);
    }
    let boxed = bytes.into_boxed_slice();
    let len = boxed.len();
    (Box::into_raw(boxed).cast::<u8>(), len)
}

/// Writes a result to `out` if it is not null.
///
/// # Safety
/// `out` must be null or valid for writes of a NovaResult.
unsafe fn write_result(out: *mut NovaResult, kind: i32, number: f64, bytes: Vec<u8>) {
    if out.is_null() {
        return;
    }
    let (bytes, len) = owned_bytes(bytes);
    // SAFETY: non-null and writable by the caller's contract; `write` does not
    // read or drop what was there (it may be uninitialized).
    unsafe {
        out.write(NovaResult {
            kind,
            number,
            bytes,
            len,
        })
    };
}

/// # Safety
/// `out` as for `write_result`.
unsafe fn write_completion(out: *mut NovaResult, completion: Completion) -> i32 {
    let (kind, number, bytes) = match completion {
        Completion::Undefined => (NOVA_VALUE_UNDEFINED, 0.0, Vec::new()),
        Completion::Number(number) => (NOVA_VALUE_NUMBER, number, Vec::new()),
        Completion::String(text) => (NOVA_VALUE_STRING, 0.0, text),
    };
    // SAFETY: forwarded contract.
    unsafe { write_result(out, kind, number, bytes) };
    NOVA_OK
}

/// # Safety
/// `out` as for `write_result`.
unsafe fn write_error(out: *mut NovaResult, error: Error) -> i32 {
    let status = match &error {
        Error::Exception(_) => NOVA_EXCEPTION,
        Error::NotAFunction(_) => NOVA_NOT_A_FUNCTION,
        Error::Interrupted => NOVA_INTERRUPTED,
        Error::OutOfMemory => NOVA_OUT_OF_MEMORY,
        Error::Dead(_) => NOVA_DEAD,
        Error::Invalid(_) => NOVA_INVALID,
    };
    let text = match error {
        Error::Exception(text) | Error::Dead(text) | Error::Invalid(text) => text,
        Error::NotAFunction(name) => name,
        other => other.to_string(),
    };
    // SAFETY: forwarded contract.
    unsafe { write_result(out, NOVA_VALUE_STRING, 0.0, text.into_bytes()) };
    status
}

/// # Safety
/// `out` as for `write_result`.
unsafe fn refuse(out: *mut NovaResult, status: i32, text: &str) -> i32 {
    // SAFETY: forwarded contract.
    unsafe { write_result(out, NOVA_VALUE_STRING, 0.0, text.as_bytes().to_vec()) };
    status
}

/// Runs `work` on the engine behind `handle`, refusing a null handle and a
/// call from inside the engine's own callback, and never unwinding into C.
///
/// # Safety
/// `handle` must be null or a live handle from `nova_engine_new` that no other
/// thread is using; `out` as for `write_result`.
unsafe fn with_engine(
    handle: *const NovaEngine,
    out: *mut NovaResult,
    work: impl FnOnce(&mut Engine) -> Result<Completion, Error>,
) -> i32 {
    if handle.is_null() {
        // SAFETY: forwarded contract.
        return unsafe { refuse(out, NOVA_INVALID, "no engine") };
    }
    // SAFETY: non-null, from `nova_engine_new` (Box::into_raw) and not freed
    // by the caller's contract; only a shared reference is made, so a nested
    // call from a callback aliases nothing mutably.
    let handle_ref = unsafe { &*handle };
    let Ok(mut engine) = handle_ref.engine.try_borrow_mut() else {
        // SAFETY: forwarded contract.
        return unsafe {
            refuse(
                out,
                NOVA_REFUSED,
                "a call into the engine was refused (it was made from inside one of its callbacks)",
            )
        };
    };
    let result = catch_unwind(AssertUnwindSafe(|| work(&mut engine)));
    drop(engine);
    let status = match result {
        // SAFETY: forwarded contract.
        Ok(Ok(completion)) => unsafe { write_completion(out, completion) },
        // SAFETY: forwarded contract.
        Ok(Err(error)) => unsafe { write_error(out, error) },
        // SAFETY: forwarded contract.
        Err(_) => unsafe { refuse(out, NOVA_DEAD, "the engine's C interface panicked") },
    };
    if handle_ref.free_requested.get() {
        // SAFETY: a callback of this call asked for the free
        // (nova_engine_free), which the borrow held by this call deferred.
        // That borrow is released, every nested call was refused, so this is
        // the outermost call and nothing else refers to the engine; the
        // pointer came from Box::into_raw, and neither it nor `handle_ref` is
        // used after this.
        let engine = unsafe { Box::from_raw(handle.cast_mut()) };
        let _ = catch_unwind(AssertUnwindSafe(move || drop(engine)));
    }
    status
}

/// Creates an engine; null on failure (with the reason in `error` if given).
///
/// # Safety
/// See `nova_engine.h`: `host` and `interrupt` (may be null) must stay
/// callable with `context` until `nova_engine_free`; `error` must be null or
/// writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nova_engine_new(
    memory_limit: usize,
    stack_limit: usize,
    host: NovaHostFn,
    interrupt: NovaInterruptFn,
    context: *mut c_void,
    error: *mut NovaResult,
) -> *mut NovaEngine {
    let Some(call) = host else {
        // SAFETY: forwarded contract.
        unsafe { refuse(error, NOVA_INVALID, "no host function callback") };
        return core::ptr::null_mut();
    };
    let time_zone = Rc::new(Cell::new(None));
    let host_time_zone = time_zone.clone();
    let created = catch_unwind(AssertUnwindSafe(|| {
        Engine::new(
            EngineOptions {
                memory_limit,
                stack_limit,
            },
            Box::new(CHost {
                call,
                interrupt,
                time_zone: host_time_zone,
                context,
            }),
        )
    }));
    match created {
        Ok(Ok(engine)) => {
            // SAFETY: forwarded contract.
            unsafe { write_result(error, NOVA_VALUE_UNDEFINED, 0.0, Vec::new()) };
            Box::into_raw(Box::new(NovaEngine {
                engine: RefCell::new(engine),
                time_zone,
                free_requested: Cell::new(false),
            }))
        }
        Ok(Err(failure)) => {
            // SAFETY: forwarded contract.
            unsafe { write_error(error, failure) };
            core::ptr::null_mut()
        }
        Err(_) => {
            // SAFETY: forwarded contract.
            unsafe { refuse(error, NOVA_DEAD, "the engine panicked while it was created") };
            core::ptr::null_mut()
        }
    }
}

/// Frees an engine (null is ignored). From inside one of the engine's own
/// callbacks the free waits until the outermost call into the engine returns;
/// until then the engine refuses every call.
///
/// # Safety
/// `handle` must be null or a live handle from `nova_engine_new`, not used
/// again afterwards (except by the calls in progress on this thread).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nova_engine_free(handle: *mut NovaEngine) {
    if handle.is_null() {
        return;
    }
    {
        // SAFETY: non-null and live by the contract; shared reference only.
        let shared = unsafe { &*handle };
        if shared.engine.try_borrow_mut().is_err() {
            shared.free_requested.set(true);
            return;
        }
    }
    // SAFETY: from Box::into_raw in nova_engine_new, live, and not borrowed by
    // any call in progress (checked above), so this is the only owner.
    let engine = unsafe { Box::from_raw(handle) };
    let _ = catch_unwind(AssertUnwindSafe(move || drop(engine)));
}

/// Defines the global `name` as a host function calling `host` with `id`.
///
/// # Safety
/// `handle` as for every engine call; `name` readable for `name_len` bytes;
/// `out` null or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nova_engine_register(
    handle: *const NovaEngine,
    name: *const u8,
    name_len: usize,
    id: u32,
    length: u32,
    mode: i32,
    out: *mut NovaResult,
) -> i32 {
    // SAFETY: forwarded contract.
    let Some(name) = (unsafe { read_utf8(name, name_len) }) else {
        // SAFETY: forwarded contract.
        return unsafe { refuse(out, NOVA_INVALID, "the name is not UTF-8") };
    };
    let mode = match mode {
        NOVA_ARGS_VALUES => ArgumentMode::Values,
        NOVA_ARGS_VALUES_AND_BYTES => ArgumentMode::ValuesAndBytes,
        // SAFETY: forwarded contract.
        _ => return unsafe { refuse(out, NOVA_INVALID, "unknown argument mode") },
    };
    // SAFETY: forwarded contract.
    unsafe {
        with_engine(handle, out, |engine| {
            engine
                .register_function(&name, id, length, mode)
                .map(|()| Completion::Undefined)
        })
    }
}

/// Evaluates UTF-8 `source` as a sloppy script (or a module).
///
/// # Safety
/// `handle` as for every engine call; `source` readable for `source_len`
/// bytes; `out` null or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nova_engine_eval(
    handle: *const NovaEngine,
    source: *const u8,
    source_len: usize,
    as_module: bool,
    out: *mut NovaResult,
) -> i32 {
    // SAFETY: forwarded contract.
    let Some(source) = (unsafe { read_utf8(source, source_len) }) else {
        // SAFETY: forwarded contract.
        return unsafe { refuse(out, NOVA_INVALID, "the source is not UTF-8") };
    };
    // SAFETY: forwarded contract.
    unsafe { with_engine(handle, out, |engine| engine.evaluate(&source, as_module)) }
}

/// A script compiled once for any number of engines (`nova_script_compile`);
/// immutable, so engines on different threads may evaluate it at once.
pub struct NovaScript {
    script: CompiledScript,
}

/// Compiles UTF-8 `source` as a sloppy script; null on failure, with the
/// reason in `error` (NOVA_EXCEPTION: a SyntaxError, as `nova_engine_eval`
/// reports it; NOVA_INVALID: code that cannot be shared between engines, to
/// be evaluated from source instead).
///
/// # Safety
/// `source` readable for `source_len` bytes; `error` null or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nova_script_compile(
    source: *const u8,
    source_len: usize,
    error: *mut NovaResult,
) -> *mut NovaScript {
    // SAFETY: forwarded contract.
    let Some(source) = (unsafe { read_utf8(source, source_len) }) else {
        // SAFETY: forwarded contract.
        unsafe { refuse(error, NOVA_INVALID, "the source is not UTF-8") };
        return core::ptr::null_mut();
    };
    match catch_unwind(AssertUnwindSafe(|| CompiledScript::compile(&source))) {
        Ok(Ok(script)) => {
            // SAFETY: forwarded contract.
            unsafe { write_result(error, NOVA_VALUE_UNDEFINED, 0.0, Vec::new()) };
            Box::into_raw(Box::new(NovaScript { script }))
        }
        Ok(Err(failure)) => {
            // SAFETY: forwarded contract.
            unsafe { write_error(error, failure) };
            core::ptr::null_mut()
        }
        Err(_) => {
            // SAFETY: forwarded contract.
            unsafe { refuse(error, NOVA_DEAD, "the engine panicked while it compiled") };
            core::ptr::null_mut()
        }
    }
}

/// Frees a compiled script (null is ignored).
///
/// # Safety
/// `script` must be null or a live pointer from `nova_script_compile` that no
/// call is using, and is not used again.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nova_script_free(script: *mut NovaScript) {
    if script.is_null() {
        return;
    }
    // SAFETY: from Box::into_raw in nova_script_compile and, by the contract,
    // live, unused by any call and freed only here, so this is its only owner.
    let script = unsafe { Box::from_raw(script) };
    let _ = catch_unwind(AssertUnwindSafe(move || drop(script)));
}

/// Evaluates a compiled script as `nova_engine_eval` evaluates its source as a
/// sloppy script.
///
/// # Safety
/// `handle` as for every engine call; `script` null or a live pointer from
/// `nova_script_compile` not freed during the call (other threads may use it
/// meanwhile); `out` null or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nova_engine_eval_script(
    handle: *const NovaEngine,
    script: *const NovaScript,
    out: *mut NovaResult,
) -> i32 {
    if script.is_null() {
        // SAFETY: forwarded contract.
        return unsafe { refuse(out, NOVA_INVALID, "no script") };
    }
    // SAFETY: non-null, from nova_script_compile and live for the call by the
    // contract; only a shared reference is made, and NovaScript is immutable
    // and Sync, so other threads reading it meanwhile alias nothing mutably.
    let script = unsafe { &*script };
    // SAFETY: forwarded contract.
    unsafe {
        with_engine(handle, out, |engine| {
            engine.evaluate_compiled(&script.script)
        })
    }
}

/// Keeps a private reference to the global function `name`. `*captured` is
/// set to whether it was a function.
///
/// # Safety
/// `handle` as for every engine call; `name` readable for `name_len` bytes;
/// `captured` and `out` null or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nova_engine_capture(
    handle: *const NovaEngine,
    name: *const u8,
    name_len: usize,
    captured: *mut bool,
    out: *mut NovaResult,
) -> i32 {
    // SAFETY: forwarded contract.
    let Some(name) = (unsafe { read_utf8(name, name_len) }) else {
        // SAFETY: forwarded contract.
        return unsafe { refuse(out, NOVA_INVALID, "the name is not UTF-8") };
    };
    let mut was_function = false;
    // SAFETY: forwarded contract.
    let status = unsafe {
        with_engine(handle, out, |engine| {
            was_function = engine.capture_global_function(&name)?;
            Ok(Completion::Undefined)
        })
    };
    if !captured.is_null() {
        // SAFETY: non-null and writable by the contract.
        unsafe { captured.write(was_function) };
    }
    status
}

/// Calls the captured (or global) function `name` with string arguments.
///
/// # Safety
/// `handle` as for every engine call; `name` readable for `name_len` bytes;
/// `args` readable for `argc` NovaStr, each readable for its length; `out`
/// null or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nova_engine_call(
    handle: *const NovaEngine,
    name: *const u8,
    name_len: usize,
    args: *const NovaStr,
    argc: usize,
    out: *mut NovaResult,
) -> i32 {
    // SAFETY: forwarded contract.
    let Some(name) = (unsafe { read_utf8(name, name_len) }) else {
        // SAFETY: forwarded contract.
        return unsafe { refuse(out, NOVA_INVALID, "the name is not UTF-8") };
    };
    if argc > NOVA_MAX_CALL_ARGS || (argc > 0 && args.is_null()) {
        // SAFETY: forwarded contract.
        return unsafe { refuse(out, NOVA_INVALID, "bad arguments") };
    }
    let mut owned = Vec::with_capacity(argc);
    for index in 0..argc {
        // SAFETY: `args` is non-null (checked) and readable for `argc` items
        // by the contract, and `index < argc`.
        let arg = unsafe { &*args.add(index) };
        // SAFETY: each item's buffer is readable for its length by the
        // contract.
        let Some(bytes) = (unsafe { read_bytes(arg.ptr, arg.len) }) else {
            // SAFETY: forwarded contract.
            return unsafe { refuse(out, NOVA_INVALID, "bad arguments") };
        };
        owned.push(String::from_utf8_lossy(&bytes).into_owned());
    }
    // SAFETY: forwarded contract.
    unsafe {
        with_engine(handle, out, |engine| {
            let args: Vec<&str> = owned.iter().map(String::as_str).collect();
            engine.call(&name, &args)
        })
    }
}

/// Whether a promise job is queued (false while the engine is busy).
///
/// # Safety
/// `handle` as for every engine call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nova_engine_has_pending_job(handle: *const NovaEngine) -> bool {
    let mut pending = false;
    // SAFETY: forwarded contract; no out-struct.
    unsafe {
        with_engine(handle, core::ptr::null_mut(), |engine| {
            pending = engine.has_pending_job();
            Ok(Completion::Undefined)
        })
    };
    pending
}

/// Runs the oldest promise job. `*ran` says whether there was one; an error
/// status means the job threw (later jobs still run).
///
/// # Safety
/// `handle` as for every engine call; `ran` and `out` null or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nova_engine_run_job(
    handle: *const NovaEngine,
    ran: *mut bool,
    out: *mut NovaResult,
) -> i32 {
    let mut did_run = false;
    // SAFETY: forwarded contract.
    let status = unsafe {
        with_engine(handle, out, |engine| {
            let has_job = engine.has_pending_job();
            let result = engine.run_pending_job();
            did_run = has_job || result.is_err();
            result.map(|_| Completion::Undefined)
        })
    };
    if !ran.is_null() {
        // SAFETY: non-null and writable by the contract.
        unsafe { ran.write(did_run) };
    }
    status
}

/// Heap bytes in use (0 while busy or dead).
///
/// # Safety
/// `handle` as for every engine call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nova_engine_memory_used(handle: *const NovaEngine) -> usize {
    let mut used = 0;
    // SAFETY: forwarded contract; no out-struct.
    unsafe {
        with_engine(handle, core::ptr::null_mut(), |engine| {
            used = engine.memory_used();
            Ok(Completion::Undefined)
        })
    };
    used
}

/// Sets the memory limit in bytes (0: none).
///
/// # Safety
/// `handle` as for every engine call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nova_engine_set_memory_limit(
    handle: *const NovaEngine,
    bytes: usize,
) -> i32 {
    // SAFETY: forwarded contract; no out-struct.
    unsafe {
        with_engine(handle, core::ptr::null_mut(), |engine| {
            engine
                .set_memory_limit(bytes)
                .map(|()| Completion::Undefined)
        })
    }
}

/// Collects garbage.
///
/// # Safety
/// `handle` as for every engine call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nova_engine_collect_garbage(handle: *const NovaEngine) -> i32 {
    // SAFETY: forwarded contract; no out-struct.
    unsafe {
        with_engine(handle, core::ptr::null_mut(), |engine| {
            engine.collect_garbage().map(|()| Completion::Undefined)
        })
    }
}

/// Gives the engine the host's local time zone: `time_zone(context,
/// epoch_seconds)` returns the seconds east of UTC at that instant (daylight
/// saving time included). Null: UTC, the default.
///
/// # Safety
/// `handle` must be null or a live handle; `time_zone` must stay callable
/// with the engine's context until the engine is freed or the function is
/// replaced.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nova_engine_set_time_zone(
    handle: *const NovaEngine,
    time_zone: NovaTimeZoneFn,
) -> i32 {
    if handle.is_null() {
        return NOVA_INVALID;
    }
    // SAFETY: non-null and live by the contract; shared reference only (the
    // setting is a Cell, so this works from inside a callback too).
    unsafe { &*handle }.time_zone.set(time_zone);
    NOVA_OK
}

/// Frees the bytes of a result and empties it; a result freed twice, or one
/// without bytes, is fine.
///
/// # Safety
/// `result` must be null, or a NovaResult this library filled (or one whose
/// `bytes` is null).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nova_result_free(result: *mut NovaResult) {
    if result.is_null() {
        return;
    }
    // SAFETY: non-null and a valid NovaResult by the contract.
    let result = unsafe { &mut *result };
    if !result.bytes.is_null() && result.len > 0 {
        let slice = core::ptr::slice_from_raw_parts_mut(result.bytes, result.len);
        // SAFETY: `bytes`/`len` came from Box::into_raw of a Box<[u8]> of that
        // length in `owned_bytes`, and are reset below so they are freed once.
        drop(unsafe { Box::from_raw(slice) });
    }
    result.bytes = core::ptr::null_mut();
    result.len = 0;
}

/// # Safety
/// `ret` must be null or the pointer passed to the running host function
/// callback.
unsafe fn set_return(ret: *mut NovaReturn, value: HostReturn) {
    if ret.is_null() {
        return;
    }
    // SAFETY: by the contract `ret` is the live, exclusive NovaReturn of the
    // callback in progress (CHost::call), which nothing else uses meanwhile.
    unsafe { (*ret).value = value };
}

/// The host function returns `null`.
///
/// # Safety
/// `ret` must be null or the pointer passed to the running host function
/// callback.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nova_return_null(ret: *mut NovaReturn) {
    // SAFETY: forwarded contract.
    unsafe { set_return(ret, HostReturn::Null) };
}

/// The host function returns a number.
///
/// # Safety
/// As `nova_return_null`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nova_return_number(ret: *mut NovaReturn, number: f64) {
    // SAFETY: forwarded contract.
    unsafe { set_return(ret, HostReturn::Number(number)) };
}

/// The host function returns a string (UTF-8, copied; invalid sequences
/// become U+FFFD).
///
/// # Safety
/// As `nova_return_null`; `ptr` readable for `len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nova_return_string(ret: *mut NovaReturn, ptr: *const u8, len: usize) {
    // SAFETY: forwarded contract.
    let Some(bytes) = (unsafe { read_bytes(ptr, len) }) else {
        return;
    };
    let text = String::from_utf8(bytes)
        .unwrap_or_else(|error| String::from_utf8_lossy(error.as_bytes()).into_owned());
    // SAFETY: forwarded contract.
    unsafe { set_return(ret, HostReturn::String(text)) };
}

/// The host function returns a new ArrayBuffer with a copy of the bytes.
///
/// # Safety
/// As `nova_return_null`; `ptr` readable for `len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nova_return_bytes(ret: *mut NovaReturn, ptr: *const u8, len: usize) {
    // SAFETY: forwarded contract.
    let Some(bytes) = (unsafe { read_bytes(ptr, len) }) else {
        return;
    };
    // SAFETY: forwarded contract.
    unsafe { set_return(ret, HostReturn::ArrayBuffer(bytes)) };
}

static VERSION: LazyLock<String> = LazyLock::new(|| {
    format!(
        "nova_embed {} (nova {})\0",
        env!("CARGO_PKG_VERSION"),
        option_env!("NOVA_EMBED_COMMIT").unwrap_or("unknown")
    )
});

/// Which limits the engine enforces: NOVA_LIMIT_INTERRUPT (the interrupt
/// callback is polled), NOVA_LIMIT_HEAP (the memory limit), NOVA_LIMIT_STACK
/// (deep recursion is a RangeError).
#[unsafe(no_mangle)]
pub extern "C" fn nova_embed_limits() -> u32 {
    nova_embed::ENFORCED_LIMITS
}

/// The library's version and the Nova commit it was built from: a
/// NUL-terminated string that lives as long as the process.
#[unsafe(no_mangle)]
pub extern "C" fn nova_embed_version() -> *const u8 {
    VERSION.as_ptr()
}

#[cfg(test)]
mod tests;
