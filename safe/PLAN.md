# Nova without `unsafe`

Goal: `#![forbid(unsafe_code)]` on `nova_vm` and `small_string`, so that no JavaScript input can reach undefined
behaviour in the engine. A widget engine built on it then has no memory corruption to turn into code execution, and
one Agent per widget keeps widgets apart.

## What `unsafe` means in this fork

Rust's meaning only: an operation that can cause undefined behaviour.

Upstream also uses `unsafe` as a "be careful" marker for the garbage collector's protocol (binding handles to a
`GcScope`, rooting, `Scoped::take`). Breaking that protocol makes a handle point at a slot that moved or was reused:
the engine reads the wrong object of the same type, or an index check panics. That is a logic error inside one
Agent's heap, never undefined behaviour, as long as every heap access is checked. So:

1. A function that is `unsafe` only because of a contract (its body performs no unsafe operation) becomes a safe
   function. Keep or reword its "## Safety" doc as "## Correctness".
2. Every real unsafe operation is rewritten with safe code: checked indexing or `get`, `try_into`, `from_le_bytes` /
   `to_le_bytes` on slices, `Vec<u8>` / `Box<[u8]>` instead of raw allocations, `expect("why")` instead of
   `unwrap_unchecked`, `unreachable!()` instead of `unreachable_unchecked`, a `match` or a constructor instead of a
   `transmute`, a safe constructor of the handle with the new lifetime instead of a lifetime `transmute`.
3. Until the last unsafe operation is gone, the intermediate states are not sound on their own (a contract-only
   function made safe may guard an unsafe operation elsewhere). That is expected on this branch; the end state is
   what is checked.
4. No `unsafe` moves into a helper crate, and no dependency is added to hide one. A dependency that keeps `unsafe`
   inside (std, hashbrown, oxc) is listed in the final report, not worked around here.
5. Behaviour must not change: test262 results, `cargo test` and the output of every corpus widget stay identical.
   A slower checked path is fine; an O(1) operation must not become O(n).

## Gate

`safe/tools/gate.sh` (about 2 minutes, longer while other agents build): builds, runs test262 against
`tests/expectations.json` (timeouts in either direction are noise under load), `cargo test`, and 501 corpus widgets
whose Nova output must hash the same as `safe/baseline/widgets-nova.json`. Heap and GC work also runs
`GATE_GC=1 safe/tools/gate.sh` (test262 with a GC between scripts, compared with
`safe/baseline/test262-gc-unexpected.txt`).

Never edit `tests/expectations.json`, `tests/metrics.json` or anything in `safe/baseline/` to make the gate pass.

Exception (the user's decision, 2026-11-14): a verified behaviour fix may update them, in its own commit that names
each entry and says why it changed. Allowed: a test262 test that goes from Fail to Pass because the engine now does
what the specification says, and a widget whose output changes and now matches QuickJS (`safe/widgets/run.py` with
`--qjs`). Never allowed: a test going from Pass to Fail, or a widget that differs from QuickJS.

Widgets whose output changes because of a fix but still differ from QuickJS for an unrelated reason (the 30 that
used to stop at a `regex parse error`) are not baselined. They are listed in
`safe/baseline/widgets-qjs-progress.json` with the number of leading items their output shares with QuickJS's, and
the gate fails if that number goes down for any of them. A compatibility fix may raise a widget's number (or move it
to `widgets-nova.json` once it matches QuickJS), in its own commit. QuickJS for that check is built from the app's
fork by `safe/tools/build-qjs.sh` into `target/qjs/`.

## Tools

- `python3 -I safe/tools/unsafe_report.py . [--files]`: what is left.
- `python3 -I safe/tools/contract_only_unsafe_fns.py`: `unsafe fn`s whose body has no unsafe operation.
- `safe/tools/gc_audit/run.sh [out-dir]` (about 2 minutes): handles that a `GcScope` function uses after a call that
  can run JavaScript without rooting them. In a scratch worktree of HEAD it binds every handle parameter at entry and
  lets the borrow checker report the uses after `gc.reborrow()`. After 281e0bfa it lists 8 functions, none a
  reachable bug: `invoke`, the Array constructor, AsyncGenerator `throw` and `Value::string_repr` are false positives;
  `ordinary_get`, `ordinary_set` and `set_default_global_bindings` only call internal methods that run no JavaScript;
  `bound_function_create` keeps `bound_this` across a Proxy's getPrototypeOf trap, which `bind` cannot reach while it
  rejects a callable Proxy. Parameters it cannot bind (generic `Self`, about 150) are not covered.
- After making functions safe: `cargo check --workspace --all-targets --message-format=json > /tmp/x.json; python3 -I
  safe/tools/strip_unused_unsafe.py /tmp/x.json; cargo fmt` removes the now unneeded `unsafe { }` around their calls
  (in any file) and the SAFETY comments that went with them. Repeat until `cargo check` is clean.

## Areas

Each area has an owner branch `safe-<area>` in its own worktree `../nova-<area>`. Edit only your files, except for
removing now-unneeded `unsafe { }` wrappers at call sites anywhere (the strip tool).

| Area | Files |
|---|---|
| a1-buffers | `types/spec/data_block.rs`, `builtins/array_buffer.rs`, `builtins/array_buffer/`, `builtins/data_view.rs`, `builtins/data_view/`, `builtins/shared_array_buffer.rs`, `builtins/structured_data/` (ArrayBuffer, SharedArrayBuffer, DataView, Atomics) |
| a2-typed-arrays | `builtins/typed_array.rs`, `builtins/typed_array/`, `builtins/indexed_collections/typed_array_objects/` |
| b-bindable | `engine/context.rs`, `engine/rootable.rs`, `engine/rootable/`, `heap/indexes.rs`, `types/language/value_vec.rs`, `types/language/object/property_key_vec.rs`, `builtins/builtin_function.rs`, and every `unsafe impl Bindable` / `Rootable` wherever it is |
| c-heap | `heap.rs`, `heap/` (not `indexes.rs`), `builtins/ordinary/shape.rs`, `types/language/object/property_storage.rs`, `builtins/array.rs`, `builtins/indexed_collections/array_objects/` |
| d-strings | `types/language/string.rs`, `types/language/string/`, `small_string/`, `types/language/bigint*`, `types/language/number*`, `types/language/object/property_key.rs`, `builtins/text_processing/`, `builtins/regexp/`, `builtins/structured_data/json_object.rs`, `abstract_operations/keyed_group.rs`, `abstract_operations/type_conversion.rs` |
| e-scripts | `scripts_and_modules/`, `builtins/ecmascript_function.rs`, `engine/bytecode/`, `ecmascript/execution/`, `builtins/finalization_registry*`, `builtins/global_object.rs`, `builtins/fundamental_objects/function_objects/`, `syntax_directed_operations/`, `types/language/function/` |

Paths are under `nova_vm/src/ecmascript/` unless they start with `engine/`, `heap` or `small_string/`.

### Round 2 (from 236 left after round 1)

The worktrees and branch names stay; the scope changes. Round 1's reports are summarised in each agent's prompt.

| Worktree / branch | Round 2 scope | Owns |
|---|---|---|
| a1-buffers | `ViewableSlice` over `DataBlock` bytes instead of `align_to`; the normal typed arrays read and write through it; `ArrayBuffer::as_slice_and_mut_slice` for two buffers | the `DataBlock` part of `data_block.rs`, `array_buffer.rs`, `array_buffer/`, `normal_typed_array.rs`, `typed_array.rs`, `typed_array_objects/` |
| a2-typed-arrays | `SharedDataBlock` as `Arc<Inner>` with atomic words, a safe `SharedSlice` replacing `RacySlice`, dropping `ecmascript_atomics` | the `SharedDataBlock` part of `data_block.rs`, `shared_array_buffer*`, `shared_typed_array.rs`, `atomics_object.rs` |
| b-bindable | `#[derive(Bindable)]` (a proc-macro crate in the workspace) replacing `bindable_handle!`; owned arguments in `ArgumentsList::with_scoped`; index-based scoped iterators | `engine/`, `heap/indexes.rs`, `builtin_function.rs`, `value_vec.rs`, `property_key_vec.rs`, the new derive crate, and one-line derive attributes on handle types anywhere |
| c-heap | `get_direct_mut` returning the stored `'static` record (all impls, including the typed arrays' two); flat strided storage in `element_array.rs`; private fields as a slice | `heap.rs`, `heap/` (not `indexes.rs`), `shape.rs`, `property_storage.rs`, `array.rs`, `array_objects/`, every `get_direct_mut` impl and its write sites, a take/restore accessor in `private_environment.rs` |
| d-strings | an in-tree WTF-8 type replacing the `wtf8` crate; `string_prototype.rs` borrows across user code; the leftovers in `small_string`, string, JSON and RegExp constructors | as in round 1, plus the `wtf8` users the replacement touches |
| e-scripts | the prepared VM stack change; `Executable` instructions as `Arc<[u8]>`; the heap sweep of realms, scripts and modules on the calling thread so their `unsafe impl Send` go; then step 1 of eager compilation (`FunctionTemplate`) | as in round 1, plus that sweep in `heap_gc.rs` |

Gate under load: set `GATE_T262_JOBS=3 GATE_JOBS=2`, and write your logs only under your own
`<scratchpad>/<worktree name>/`.

### Round 4 (from 0 unsafe, 2026-11-14): make it the widget engine

Measured against QuickJS on the 501 widgets (release builds, `safe/widgets/run.py --qjs`): 360 same output, 140
differ (many from a few Nova bugs, the most common `ReferenceError: Uninitialized this binding`), Nova CPU 100 s
against 23 s (median widget 2.9x, heavy animations 9-10x). Tracks, one worktree each:

| Worktree / branch | Track |
|---|---|
| e-scripts | compat: the differences against QuickJS, by root cause |
| c-heap | perf: the hot paths of the heavy widgets, output unchanged |
| b-bindable | limits: interrupt (uncatchable termination), heap limit, call depth and native stack, parse and JSON depth, size caps |
| d-strings | regexp: an in-tree JS regex engine (regress fork, no unsafe, step budget, UTF-16), drop `regex` |
| a2-typed-arrays | dependencies: JSON, number parsing/printing and byte search without unsafe crates; audit the rest |
| a1-buffers | embedding: a safe `nova_embed` API, a minimal C ABI crate, the xcframework build, then the Swift backend in `../widget-sandbox-nova` |

The embedding contract the limits track provides (names may be refined, keep them in sync):
- `HostHooks::check_interrupt(&self) -> bool` (default false), polled by the interpreter at least every few
  thousand instructions and inside long native loops; true ends the running script with an uncatchable
  termination that `try`/`catch`/`finally` cannot stop.
- `Agent::set_heap_limit(bytes)` / `Agent::heap_size()`: allocation past the limit ends the script the same way,
  with a memory reason.
- a maximum call depth and a native-stack guard that throw a catchable RangeError instead of overflowing.

## Commits

Small steps, each passing the gate. Commit as the repository's configured identity (`tmacos <>`) with
`TZ=Asia/Dubai git commit`. No Co-Authored-By or "Generated with" lines. First line: what changed, imperative. Body:
why the new code cannot reach undefined behaviour, and the unsafe count before and after.

## Engine bugs found along the way

Fixed (each in its own commit, results checked against QuickJS):
- `unescape` advanced by 1 instead of to the match (`unescape("%41%42%43")` gave `"AB%43"`); `notes-storage.html`
  panicked in `wtf8`. e2f45041.
- `%TypedArray%.prototype.slice` with a species view of another element type over the same buffer: the source index
  used the target's element size (wrong values or a panic), floats were converted to integers with `as u64`, and the
  copy read through a raw pointer while writing through `&mut` to the same bytes (aliasing UB). fb3b1398, 1f8baa76.
- `Array.prototype.slice` panicked when the species constructor shrank the source. b72eb433.
- Sorting a growable shared typed array with a comparator that grows the buffer panicked. 1505a469.
- `ElementArrays::reserve_elements_raw` E1 arms read the wrong store (unreachable). a76a156a.
- Dangling pointers read by the next GC after a panic the embedder catches: `vm_stack` (45409a77); the interpreter's
  `&'static` bytecode slice could outlive its Executable (e6a2c978); unreachable realms/scripts/modules dropped their
  host `Rc` on GC worker threads (024cc498).

- `RegExp.prototype.flags` wrote "v" for "y" and lost "m". 94ad8292 (7 test262 tests pass).
- `slice`/`substring`/`substr` lost lone surrogates and panicked on an index inside a surrogate pair; `substr` sliced
  UTF-8 with UTF-16 indices. 3881720b.
- Two small strings concatenated didn't join a surrogate pair. 21c508cc (3 test262 tests pass).
- RegExp exec/replace/split/search/match used UTF-8 positions as UTF-16 ones, GetSubstitution expanded only templates
  starting with "$" and panicked on `$'`, a functional replacer didn't get the string, empty matches inside a lone
  surrogate panicked. a9b69dba (26 test262 tests pass).
- RegExp captures went through `from_utf8_lossy` (unreachable today: the regex crate never matches surrogate bytes).
  ad18c376.
- `indexOf`/`lastIndexOf`/`includes`/`startsWith`/`endsWith` panicked on a position inside a surrogate pair; string
  `split` lost lone surrogates and split "😀" by "" into one element. ad1aaacb.
- `replace`/`replaceAll` with a string pattern ignored `$` patterns and passed a byte offset to a replacer.
  30544066 (7 test262 tests pass).
- RegExp named groups were never filled in. 090e2895 (8 test262 tests pass).
- The RegExp `m` flag did nothing. a9d7c685 (13 test262 tests pass).
- RegExp `\d`, `\w`, `\b` were Unicode-aware. 864d6ad2 (8 test262 tests pass).

Open (each changes test262 or widget results, so each needs its own commit):
- RegExp: `.` and negated classes never match a lone surrogate and a lone surrogate in a pattern is a syntax error (the
  regex crate needs valid UTF-8); multi-line `^`/`$` only treat "\n" as a line terminator; `\s` is Unicode
  White_Space (takes U+0085, misses U+FEFF); `i` without `u` folds non-ASCII into ASCII (`/s/i` matches "ſ"); no
  backreferences or lookaround; `indices` (`d` flag) is not created.
- `toUpperCase`/`toLowerCase`/`normalize`/`repeat`/`localeCompare` go through `to_string_lossy` (lone surrogates
  become U+FFFD).
- `ArgumentsList::with_scoped` keeps a raw pointer to the caller's frame in the GC roots; a caught panic leaves it
  dangling (b-bindable round 2).
