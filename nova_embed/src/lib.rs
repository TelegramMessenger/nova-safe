// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! # A safe API for running untrusted scripts in Nova
//!
//! One [`Engine`] is one Nova agent with one realm. Everything that crosses
//! this API is an owned Rust value: script source, strings, numbers and the
//! bytes of typed arrays go in and come out as copies, so no handle into the
//! engine's heap ever leaves it.
//!
//! - Host functions are registered by name with an id chosen by the embedder.
//!   Each one is a builtin function whose behaviour is one of a fixed set of
//!   Rust functions; which one it is gives its slot, and the slot gives the
//!   id. Nothing about the id is stored in the JavaScript heap, so a script
//!   can neither read nor forge it.
//! - A panic anywhere inside the engine is caught at this API's boundary. The
//!   engine is then dead: its agent is dropped and every later call returns
//!   [`Error::Dead`] without touching it again.
//! - The engine never calls back into the embedder except through its
//!   [`Host`]: a host function call, or the interrupt check.

#![forbid(unsafe_code)]

mod hooks;
mod limits;
mod trampolines;
mod values;
mod web;

use std::{
    cell::RefCell,
    collections::{HashMap, VecDeque},
    panic::{AssertUnwindSafe, catch_unwind},
    rc::Rc,
};

use nova_vm::{
    ecmascript::{
        Agent, AgentOptions, Behaviour, BuiltinFunctionArgs, CompiledScript as VmCompiledScript,
        Function, GcAgent, InternalMethods, Job, Object, PropertyDescriptor, PropertyKey,
        RealmRoot, String as JsString, Value, create_builtin_function, parse_module, parse_script,
        script_evaluation,
    },
    engine::{Bindable, GcScope, Global, Scopable},
};

pub use limits::{ENFORCED as ENFORCED_LIMITS, LIMIT_HEAP, LIMIT_INTERRUPT, LIMIT_STACK};
pub use values::{Completion, HostArg, HostReturn, TypedArrayKind};

use hooks::{EntryGuard, HOOKS};

/// The most host functions one engine can register.
pub const MAX_HOST_FUNCTIONS: usize = trampolines::SLOTS;

/// The most arguments a host function receives; later ones are ignored.
pub const MAX_HOST_ARGUMENTS: usize = 8;

/// What the engine asks of its embedder. Both methods run on the thread that
/// called into the engine, inside that call.
pub trait Host {
    /// A script called the host function registered with `id`.
    fn call(&self, id: u32, args: Vec<HostArg>) -> HostReturn;

    /// Polled while scripts run; `true` ends the running script with an
    /// uncatchable termination ([`Error::Interrupted`]).
    fn should_interrupt(&self) -> bool {
        false
    }

    /// Seconds east of UTC of the local time zone at `epoch_seconds`,
    /// daylight saving time included: Date's local time. The default is UTC.
    fn local_time_zone_offset(&self, epoch_seconds: i64) -> i32 {
        let _ = epoch_seconds;
        0
    }
}

/// Limits fixed when the engine is created.
#[derive(Debug, Clone, Copy)]
pub struct EngineOptions {
    /// Heap bytes a script may use before it is ended with
    /// [`Error::OutOfMemory`]; 0 means no limit. One allocation the limit has
    /// no room for is a RangeError unless
    /// [`Engine::set_out_of_memory_fatal`] says otherwise.
    pub memory_limit: usize,
    /// Native stack bytes JavaScript calls may use before a `RangeError`;
    /// 0 means the engine's default.
    pub stack_limit: usize,
}

/// Why a call into the engine did not complete normally.
#[derive(Debug, Clone, PartialEq)]
pub enum Error {
    /// The script threw; the text is `name: message` for error objects.
    Exception(String),
    /// [`Engine::call`] named something that is not a function.
    NotAFunction(String),
    /// [`Host::should_interrupt`] ended the script.
    Interrupted,
    /// The script went past the memory limit.
    OutOfMemory,
    /// The engine panicked, now or earlier; it never runs again.
    Dead(String),
    /// The request was refused before the engine ran (bad name, too many
    /// host functions).
    Invalid(String),
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Error::Exception(text) => f.write_str(text),
            Error::NotAFunction(name) => write!(f, "{name} is not a function"),
            Error::Interrupted => f.write_str("InternalError: interrupted"),
            Error::OutOfMemory => f.write_str("InternalError: out of memory"),
            Error::Dead(reason) => write!(f, "engine fault: {reason}"),
            Error::Invalid(reason) => f.write_str(reason),
        }
    }
}

/// How a registered host function receives typed arrays.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArgumentMode {
    /// Strings and numbers; anything else arrives as [`HostArg::Other`].
    Values,
    /// Also copies the viewed bytes of typed array arguments
    /// ([`HostArg::Bytes`]).
    ValuesAndBytes,
}

#[derive(Clone, Copy)]
pub(crate) struct Slot {
    id: u32,
    mode: ArgumentMode,
}

/// The engine's state that its host hooks and host functions reach while it
/// runs (see `hooks::current`).
pub(crate) struct Shared {
    host: Box<dyn Host>,
    jobs: RefCell<VecDeque<Job>>,
    slots: RefCell<Vec<Slot>>,
}

struct Live {
    agent: GcAgent,
    realm: RealmRoot,
    captured: HashMap<std::string::String, Global<Function<'static>>>,
}

enum State {
    Live(Box<Live>),
    Dead(std::string::String),
}

/// One Nova agent and realm running untrusted scripts.
pub struct Engine {
    state: State,
    shared: Rc<Shared>,
    options: EngineOptions,
}

fn panic_text(payload: &(dyn core::any::Any + Send)) -> std::string::String {
    if let Some(text) = payload.downcast_ref::<&str>() {
        format!("the engine panicked: {text}")
    } else if let Some(text) = payload.downcast_ref::<std::string::String>() {
        format!("the engine panicked: {text}")
    } else {
        "the engine panicked".to_owned()
    }
}

impl Engine {
    /// Creates an engine with one realm, the standard globals, and `atob` and
    /// `btoa` as a QuickJS context has them.
    pub fn new(options: EngineOptions, host: Box<dyn Host>) -> Result<Engine, Error> {
        let shared = Rc::new(Shared {
            host,
            jobs: RefCell::new(VecDeque::new()),
            slots: RefCell::new(Vec::new()),
        });
        let guard = EntryGuard::enter(shared.clone());
        let created = catch_unwind(AssertUnwindSafe(|| {
            let mut agent = GcAgent::new(
                AgentOptions {
                    disable_gc: false,
                    print_internals: false,
                    no_block: true,
                },
                &HOOKS,
            );
            let realm = agent.create_realm(
                None::<for<'a> fn(&mut Agent, GcScope<'a, '_>) -> Object<'a>>,
                None::<for<'a> fn(&mut Agent, GcScope<'a, '_>) -> Object<'a>>,
                None::<fn(&mut Agent, Object, GcScope)>,
            );
            agent.run_in_realm(&realm, web::install);
            let mut live = Box::new(Live {
                agent,
                realm,
                captured: HashMap::new(),
            });
            limits::apply(&mut live.agent, options);
            live
        }));
        drop(guard);
        match created {
            Ok(live) => Ok(Engine {
                state: State::Live(live),
                shared,
                options,
            }),
            Err(payload) => Err(Error::Dead(panic_text(&*payload))),
        }
    }

    /// Why the engine is dead, if it is.
    pub fn dead_reason(&self) -> Option<&str> {
        match &self.state {
            State::Live(_) => None,
            State::Dead(reason) => Some(reason),
        }
    }

    /// The limits the engine was created with (the memory limit as last set).
    pub fn options(&self) -> EngineOptions {
        self.options
    }

    /// Runs `work` on the live engine with its hooks entered; a panic kills
    /// the engine.
    fn enter<R>(
        &mut self,
        work: impl FnOnce(&mut Live, &Shared) -> Result<R, Error>,
    ) -> Result<R, Error> {
        let live = match &mut self.state {
            State::Live(live) => live,
            State::Dead(reason) => return Err(Error::Dead(reason.clone())),
        };
        let guard = EntryGuard::enter(self.shared.clone());
        let shared = &*self.shared;
        let result = catch_unwind(AssertUnwindSafe(|| work(live, shared)));
        drop(guard);
        match result {
            Ok(result) => result,
            Err(payload) => {
                let reason = panic_text(&*payload);
                self.kill(reason.clone());
                Err(Error::Dead(reason))
            }
        }
    }

    /// Drops the agent without running anything in it again.
    fn kill(&mut self, reason: std::string::String) {
        let state = core::mem::replace(&mut self.state, State::Dead(reason));
        self.shared.jobs.borrow_mut().clear();
        if let State::Live(live) = state {
            let _ = catch_unwind(AssertUnwindSafe(move || drop(live)));
        }
    }

    /// Defines the global `name` as a host function that calls
    /// [`Host::call`] with `id`. The global is an ordinary writable,
    /// configurable data property, so a script can delete it.
    pub fn register_function(
        &mut self,
        name: &str,
        id: u32,
        length: u32,
        mode: ArgumentMode,
    ) -> Result<(), Error> {
        let slot_index = self.shared.slots.borrow().len();
        let Some(behaviour) = trampolines::behaviour(slot_index) else {
            return Err(Error::Invalid(format!(
                "more than {MAX_HOST_FUNCTIONS} host functions"
            )));
        };
        let Some(static_name) = names::intern(name) else {
            return Err(Error::Invalid(
                "a host function name must be 1 to 64 bytes".to_owned(),
            ));
        };
        self.enter(|live, shared| {
            let realm = &live.realm;
            live.agent.run_in_realm(realm, |agent, mut gc| {
                let function = create_builtin_function(
                    agent,
                    Behaviour::Regular(behaviour),
                    BuiltinFunctionArgs::new(length.min(255), static_name),
                    gc.nogc(),
                );
                let value: Value = function.unbind().into();
                let key = PropertyKey::from_str(agent, name, gc.nogc());
                let global = agent.current_realm(gc.nogc()).global_object(agent);
                let defined = global
                    .unbind()
                    .internal_define_own_property(
                        agent,
                        key.unbind(),
                        PropertyDescriptor::new_data_descriptor(value),
                        gc.reborrow(),
                    )
                    .unwrap_or(false);
                if !defined {
                    return Err(Error::Invalid(format!(
                        "the global {name} cannot be defined"
                    )));
                }
                shared.slots.borrow_mut().push(Slot { id, mode });
                Ok(())
            })
        })
    }

    /// Evaluates `source` as a sloppy global script, or as a module (which
    /// may not import anything), and returns its completion value.
    pub fn evaluate(&mut self, source: &str, as_module: bool) -> Result<Completion, Error> {
        self.enter(|live, shared| {
            let realm = &live.realm;
            live.agent.run_in_realm(realm, |agent, mut gc| {
                let source_text = JsString::from_str(agent, source, gc.nogc());
                let realm = agent.current_realm(gc.nogc());
                let result = if as_module {
                    let module = match parse_module(agent, source_text, realm, None, gc.nogc()) {
                        Ok(module) => module,
                        Err(errors) => return Err(parse_error(agent, &errors)),
                    };
                    agent
                        .run_module(module.unbind(), None, gc.reborrow())
                        .unbind()
                        .bind(gc.nogc())
                } else {
                    let script =
                        match parse_script(agent, source_text, realm, false, None, gc.nogc()) {
                            Ok(script) => script,
                            Err(errors) => return Err(parse_error(agent, &errors)),
                        };
                    script_evaluation(agent, script.unbind(), gc.reborrow())
                        .unbind()
                        .bind(gc.nogc())
                };
                values::completion(agent, shared, result.unbind(), gc)
            })
        })
    }

    /// Evaluates a script compiled by [`CompiledScript::compile`]: the same as
    /// [`Engine::evaluate`] of its source as a sloppy script, without parsing
    /// or compiling it again.
    pub fn evaluate_compiled(&mut self, script: &CompiledScript) -> Result<Completion, Error> {
        self.enter(|live, shared| {
            let realm = &live.realm;
            live.agent.run_in_realm(realm, |agent, mut gc| {
                let realm = agent.current_realm(gc.nogc());
                let script = script.script.instantiate(agent, realm, None, gc.nogc());
                let result = script_evaluation(agent, script.unbind(), gc.reborrow())
                    .unbind()
                    .bind(gc.nogc());
                values::completion(agent, shared, result.unbind(), gc)
            })
        })
    }

    /// Keeps a reference to the global function `name`: [`Engine::call`] of
    /// that name uses it from then on, even after the global is deleted or
    /// replaced. Returns `false` when the global is not a function.
    pub fn capture_global_function(&mut self, name: &str) -> Result<bool, Error> {
        self.enter(|live, shared| {
            let realm = &live.realm;
            let captured = &mut live.captured;
            live.agent.run_in_realm(realm, |agent, mut gc| {
                let value = match values::get_global(agent, name, gc.reborrow()) {
                    Ok(value) => value.unbind(),
                    Err(error) => {
                        return Err(values::error(agent, shared, error.unbind().value(), gc));
                    }
                };
                let Ok(function) = Function::try_from(value) else {
                    return Ok(false);
                };
                let global = Global::new(agent, function.unbind());
                if let Some(old) = captured.insert(name.to_owned(), global) {
                    let _ = old.take(agent);
                }
                Ok(true)
            })
        })
    }

    /// Calls the captured function `name` (or else the global of that name)
    /// with string arguments and `undefined` as `this`.
    pub fn call(&mut self, name: &str, args: &[&str]) -> Result<Completion, Error> {
        let args: Vec<&[u8]> = args.iter().map(|arg| arg.as_bytes()).collect();
        self.call_wtf8(name, &args)
    }

    /// [`Engine::call`] with arguments as WTF-8 (see [`HostArg::String`]), so
    /// that a string a script handed out goes back in with its lone
    /// surrogates; bytes that are not WTF-8 become U+FFFD.
    pub fn call_wtf8(&mut self, name: &str, args: &[&[u8]]) -> Result<Completion, Error> {
        self.enter(|live, shared| {
            let realm = &live.realm;
            let captured = &live.captured;
            live.agent.run_in_realm(realm, |agent, mut gc| {
                let callee = if let Some(global) = captured.get(name) {
                    global.get(agent, gc.nogc()).into()
                } else {
                    match values::get_global(agent, name, gc.reborrow()) {
                        Ok(value) => value.unbind(),
                        Err(error) => {
                            return Err(values::error(agent, shared, error.unbind().value(), gc));
                        }
                    }
                };
                let Ok(function) = Function::try_from(callee) else {
                    return Err(Error::NotAFunction(name.to_owned()));
                };
                let function = function.scope(agent, gc.nogc());
                let mut arguments: Vec<Value<'static>> = args
                    .iter()
                    .map(|arg| {
                        values::string_from_wtf8(agent, arg, gc.nogc())
                            .unbind()
                            .into()
                    })
                    .collect();
                let result = function
                    .get(agent)
                    .call(agent, Value::Undefined, &mut arguments, gc.reborrow())
                    .unbind()
                    .bind(gc.nogc());
                values::completion(agent, shared, result.unbind(), gc)
            })
        })
    }

    /// Whether a promise job is queued.
    pub fn has_pending_job(&self) -> bool {
        matches!(self.state, State::Live(_)) && !self.shared.jobs.borrow().is_empty()
    }

    /// Runs the oldest queued promise job. `Ok(false)`: nothing was queued;
    /// `Err(Error::Exception)`: the job threw (the queue goes on).
    pub fn run_pending_job(&mut self) -> Result<bool, Error> {
        let Some(job) = self.shared.jobs.borrow_mut().pop_front() else {
            return match &self.state {
                State::Live(_) => Ok(false),
                State::Dead(reason) => Err(Error::Dead(reason.clone())),
            };
        };
        // A promise job runs in the engine's realm, as a microtask after a
        // script: its own realm, when it has one, is entered by `Job::run`. A
        // job without one (a reaction whose handler was not callable) is
        // valid; `GcAgent::run_job` is for macrotasks and requires one.
        self.enter(|live, shared| {
            let realm = &live.realm;
            live.agent.run_in_realm(realm, |agent, mut gc| {
                match job.run(agent, gc.reborrow()).unbind().bind(gc.nogc()) {
                    Ok(()) => Ok(true),
                    Err(error) => Err(values::error(agent, shared, error.value().unbind(), gc)),
                }
            })
        })
    }

    /// Collects garbage now.
    pub fn collect_garbage(&mut self) -> Result<(), Error> {
        self.enter(|live, _| {
            live.agent.gc();
            Ok(())
        })
    }

    /// Bytes of the engine's heap in use (0 for a dead engine).
    pub fn memory_used(&mut self) -> usize {
        match &mut self.state {
            State::Live(live) => {
                catch_unwind(AssertUnwindSafe(|| limits::heap_size(&live.agent))).unwrap_or(0)
            }
            State::Dead(_) => 0,
        }
    }

    /// Changes the memory limit; 0 means none.
    pub fn set_memory_limit(&mut self, bytes: usize) -> Result<(), Error> {
        self.options.memory_limit = bytes;
        self.enter(|live, _| {
            limits::set_heap_limit(&mut live.agent, bytes);
            Ok(())
        })
    }

    /// Chooses what one allocation the memory limit has no room for does. By
    /// default it throws a RangeError: caught, the script goes on just under
    /// the limit; uncaught, the call ends with [`Error::OutOfMemory`]. With
    /// `fatal` it ends the script with [`Error::OutOfMemory`] wherever it
    /// happens, as going over the limit does: no `catch`, promise reaction
    /// or getter keeps the script running.
    pub fn set_out_of_memory_fatal(&mut self, fatal: bool) -> Result<(), Error> {
        self.enter(|live, _| {
            limits::set_out_of_memory_fatal(&mut live.agent, fatal);
            Ok(())
        })
    }
}

/// A sloppy script compiled once, for [`Engine::evaluate_compiled`] in any
/// number of engines, on any thread. Immutable once compiled; it holds no
/// handle into any engine and cannot be made from bytes.
pub struct CompiledScript {
    script: VmCompiledScript,
}

struct NoHost;

impl Host for NoHost {
    fn call(&self, _id: u32, _args: Vec<HostArg>) -> HostReturn {
        HostReturn::Undefined
    }
}

impl CompiledScript {
    /// Parses and compiles `source` as a sloppy global script in an engine
    /// of its own, which never runs it. A syntax error is
    /// [`Error::Exception`], as [`Engine::evaluate`] reports it; code that
    /// cannot be shared between engines is [`Error::Invalid`], and the
    /// caller evaluates the source instead.
    pub fn compile(source: &str) -> Result<CompiledScript, Error> {
        let mut engine = Engine::new(
            EngineOptions {
                memory_limit: 0,
                stack_limit: 0,
            },
            Box::new(NoHost),
        )?;
        let script = engine.enter(|live, _| {
            let realm = &live.realm;
            live.agent.run_in_realm(realm, |agent, gc| {
                let source_text = JsString::from_str(agent, source, gc.nogc());
                let realm = agent.current_realm(gc.nogc());
                let script = match parse_script(agent, source_text, realm, false, None, gc.nogc()) {
                    Ok(script) => script,
                    Err(errors) => return Err(syntax_error(&errors)),
                };
                VmCompiledScript::export(agent, script, gc.nogc())
                    .map_err(|error| Error::Invalid(error.to_string()))
            })
        })?;
        Ok(CompiledScript { script })
    }
}

/// The error of a failed parse: the termination when the interrupt stopped
/// it, else a SyntaxError.
fn parse_error<E: core::fmt::Display>(agent: &mut Agent, errors: &[E]) -> Error {
    limits::take_termination(agent).unwrap_or_else(|| syntax_error(errors))
}

fn syntax_error<E: core::fmt::Display>(errors: &[E]) -> Error {
    let message = errors
        .first()
        .map(|error| error.to_string())
        .unwrap_or_else(|| "invalid script".to_owned());
    Error::Exception(format!("SyntaxError: {message}"))
}

mod names {
    use std::{collections::HashSet, sync::Mutex};

    /// Distinct host function names leaked for the process's life; builtin
    /// function names must be `'static`.
    const MAX_NAMES: usize = 256;

    static NAMES: Mutex<Option<HashSet<&'static str>>> = Mutex::new(None);

    pub(crate) fn intern(name: &str) -> Option<&'static str> {
        if name.is_empty() || name.len() > 64 {
            return None;
        }
        let mut names = NAMES
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let names = names.get_or_insert_with(HashSet::new);
        if let Some(found) = names.get(name) {
            return Some(found);
        }
        if names.len() >= MAX_NAMES {
            return Some("");
        }
        let leaked: &'static str = Box::leak(name.to_owned().into_boxed_str());
        names.insert(leaked);
        Some(leaked)
    }
}

#[cfg(test)]
mod tests;
