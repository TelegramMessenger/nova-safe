# Security model of the Nova widget engine

This fork of [Nova](https://github.com/trynova/nova) runs untrusted widget scripts inside Telegram for macOS. This
file states what protects the process from a hostile script, what is trusted, and what is left.

## Layers

1. **No memory-unsafe code in the engine.** `nova_vm`, `small_string`, `wtf8`, `regress` (our fork of the regress
   RegExp engine), `nova_vm_derive` and `nova_embed` carry `#![forbid(unsafe_code)]`; the compiler rejects any
   `unsafe` in them (`nova_vm` only allows it with the opt-in `probes` feature, for DTrace probe macros; release
   builds for the app leave it off). A logic bug in the engine can make a script compute a wrong value, throw, or
   panic; it cannot read or write memory it does not own. `safe/tools/unsafe_report.py` counts what is left: 0.
2. **A small, audited C ABI.** `nova_embed_ffi` is the only crate with `unsafe` (`#![deny(unsafe_op_in_unsafe_fn)]`,
   every block commented with its precondition): byte buffers are copied into owned Rust values at once, results are
   owned by the caller and freed once, re-entry from a host callback is refused, every entry point runs under
   `catch_unwind` and a panic marks the engine dead (never touched again). Only `nova_*` symbols are exported.
3. **One engine per widget, no shared state between engines.** Each widget has its own Agent and heap. Host function
   ids live in Rust, never in the JS heap; the hook stack is per call and per thread.
4. **Resource limits** (so a script can only hurt itself): an interrupt polled every 4096 instructions and inside
   long native loops ends a script with a termination no `try`/`catch`/`finally` can stop; a heap limit; a native
   stack guard that turns deep recursion (JS calls, bound functions, Proxy chains, JSON, `flat`, generators) into a
   catchable `RangeError`; a nesting pre-scan that turns too-deep source into a `SyntaxError` before the parser runs;
   huge single allocations refused before allocating; RegExp matching with a step budget (no ReDoS hang).
5. **The XPC sandbox.** Widgets run in `WidgetEngineService.xpc`, sandboxed with no file or network access and no
   JIT entitlement; the app re-validates everything a widget sends.

## What is trusted (the trusted computing base)

- `rustc`/LLVM and Rust's `std` (including `hashbrown`, which is std's own HashMap).
- The dependencies listed in `safe/TCB.md` with their `unsafe` counts. The largest is `oxc` (the JavaScript parser),
  which parses script source; its own recursion is bounded by our nesting pre-scan. Replacing or auditing it is the
  biggest remaining item.
- `nova_embed_ffi` and the Swift wrapper `NovaEngine.swift` in widget-sandbox.
- macOS's sandbox for the XPC service.

## Removed from the trusted base during this work

- 1144 `unsafe` sites in the engine itself.
- `regex`, `regex-automata`, `regex-syntax`, `aho-corasick` (replaced by the in-tree regress fork).
- `sonic-rs` and its dependencies (in-tree JSON parser), `ryu-js`, `lexical`, `fast-float` (std-based number
  conversion), `memchr` (in-tree Two-Way search), `ahash`, `rand` and their dependencies (std SipHash, per-agent
  xorshift seeded from OS randomness), and in the widget build `temporal_rs` and SharedArrayBuffer/Atomics.
- A memory-safety bug was found in the removed `ryu-js`: `(4.2025059341068935e-46).toFixed(65)` returned bytes of an
  uninitialized stack buffer into a script string. Upstream Nova still has it.

## What remains (honestly)

- A bug in `rustc`, LLVM, `std` or a listed dependency could still break the guarantees; the sandbox is the wall
  behind that.
- Denial of service inside one widget (it burns its own CPU quota until the interrupt stops it).
- Correctness differences against QuickJS (see PLAN.md); they change what a widget shows, not what it can reach.
