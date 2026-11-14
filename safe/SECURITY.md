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
  conversion), `memchr` (in-tree Two-Way search), `ahash`, `rand` and their dependencies (std SipHash, an in-tree
  hash under random per-map keys for maps keyed by engine handles, per-agent xorshift seeded from OS randomness), and
  in the widget build `temporal_rs` and SharedArrayBuffer/Atomics. A script can collide handle keys only by chance;
  a flood slows only its own Agent, and the CPU limit ends it. Object Shape transition tables hash a property key by
  identity (an injective 64-bit packing of small-string bytes, integer value, or heap index) with the same hash
  under per-Agent random keys, and are rehashed when a garbage collection moves their keys.
- A memory-safety bug was found in the removed `ryu-js`: `(4.2025059341068935e-46).toFixed(65)` returned bytes of an
  uninitialized stack buffer into a script string. Upstream Nova still has it.

## How it was tested

- test262 (53,616 tests) against recorded expectations, with and without a garbage collection between scripts; a
  test may only move from Fail to Pass, in a commit of its own (`safe/PLAN.md`).
- The Telegram widget corpora (about 500 widgets) through `safe/widgets/run.py`: Nova's output must not change, and is
  compared with QuickJS's (the app's previous engine).
- A hostile-input hunt: generators and fuzzers for user code running inside native builtins, forced collections
  inside every callback, typed-array resize/detach matrices, Proxy re-entrancy, size and nesting extremes. Its
  findings (allocation-size aborts, loops the interrupt did not reach, handles left unrooted across user code, and
  several unwraps) are fixed with regression tests (`limits_tests.rs`, `gc_rooting_tests.rs`,
  `panic_regression_tests.rs`).
- `safe/tools/gc_audit/`: makes the borrow checker list every handle parameter used after a call that can run
  JavaScript, plus a runtime sweep of every builtin with arguments that force a collection.
- A review of the C ABI and the Swift wrapper (no memory-safety finding) and the C ABI under ASan/UBSan.

## What remains (honestly)

- A bug in `rustc`, LLVM, `std` or a listed dependency could still break the guarantees; the sandbox is the wall
  behind that.
- Denial of service inside one widget (it burns its own CPU quota until the interrupt stops it).
- Correctness differences against QuickJS: on the 501 corpus widgets, 497 produce exactly QuickJS's output; the other
  three differ in an error's stack text, in nova_cli's module-loader message, and in one case where Nova follows the
  specification and QuickJS does not (`regexp-delete-recompile`). They change what a widget shows, not what it can
  reach.
- Speed and memory: Nova is slower than QuickJS (about 2x on a median widget, 6-8x on the heaviest animations; a
  pooled widget engine boots in about 22 ms against 8 ms) and keeps about 5.4 MiB of live malloc per engine after the
  sandbox bundle. Widgets stay inside the CPU limits; the heaviest test widgets use up to ~80% of the 500 ms task
  limit.
- The parser (oxc) is trusted code with `unsafe`, bounded by the nesting pre-scan; a fork or audit is the next step.
