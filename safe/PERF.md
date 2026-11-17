# Performance against QuickJS

Nova replaces QuickJS as the widget engine, so QuickJS is the bar. This file says where Nova is slower, why, what has
been done about it and what is left, in the order that matters for widgets. Nothing here may add `unsafe`: every fix
below is ordinary Rust.

## How it is measured

`safe/bench` runs the same scripts in both engines, each embedded as the widget sandbox embeds its engine (`bench.c`:
an 8 MB heap, an interrupt callback that reads a clock, a 6 MB stack limit on an 8 MB thread). One process runs one
case, so no case sees another's heap; a case is timed until it has run for 300 ms and the fastest run counts.

```bash
safe/bench/run.py --qjs-src <a QuickJS checkout> [--nova-lib <libnova_embed_ffi.a>] [--filter loop/] [--label NAME] [--baseline results.json]
```

Without `--nova-lib` it builds the library from this checkout (release, as a static library alone, so that the
profile's fat LTO applies). `--baseline` adds a column against an earlier result, which is how a change is judged.
108 cases in `safe/bench/cases`: the interpreter (`loop/`), calls, properties, arrays, strings, numbers and typed
arrays, collections, JSON and RegExp, garbage under the small heap, and small whole programs (`app/`).

A case that the engine ends (out of memory, the interrupt) is reported as failed, not timed: under the widget limits
that is the result that matters.

## Where it stands

Nova's time over QuickJS's, geometric mean per group, on an Apple M-series machine (QuickJS fork `fbc91d8`, `-O2`;
Nova release with fat LTO, rustc 1.96.1). "Before" is `c13a0dfc` with the embedding fixes; "now" has the first round
of changes below.

| Group | Cases | Before | Now |
|---|---|---|---|
| loop | 11 | 11.1x | 7.5x |
| call | 14 | 10.4x | 8.9x |
| typed | 4 | 10.6x | 8.1x |
| math | 4 | 9.7x | 7.8x |
| prop | 13 | 7.1x | 6.1x |
| string | 26 | 10.6x | 5.9x |
| app | 8 | 6.1x | 5.4x |
| array | 14 | 5.8x | 5.2x |
| map, set | 4 | 4.6x | 3.8x |
| gc | 3 of 5 | 3.6x | 2.8x |
| regexp | 4 | 2.4x | 2.2x |
| json | 2 | 1.0x | 0.9x |
| all | 108 | 7.6x | 5.7x |

The worst case was 303x (appending to a string 100,000 times) and is 38x. One case ran Nova out of memory before
(`prop/destructuring`) and none does now. QuickJS runs out of its 8 MB on two cases Nova fits (30,000 retained
objects, 50,000 closures): Nova's heap is the more compact one.

What is written in Rust or C on both sides is level: JSON, RegExp, `indexOf`, `split`/`join`, `sort`. The gap is in
running JavaScript itself.

## The areas, and how far each can go

Reach figures are estimates from the profiles, not measurements.

### 1. Variables that are not locals of the running function

Every read or write of a closure variable, a module-level variable or a global resolves the name at run time:
`ResolveBinding` walks the environment chain, looks the name up in each record (`BindingMap::position`), builds a
Reference, and `GetValue`/`PutValue` goes through it. A loop over a closure variable is 12.6x, over a global 13.1x,
and since widget code and the sandbox bundle are closures over a module scope, this is under every other number
here. QuickJS resolves these when it compiles, to an index.

Next: resolve at compile time to (environments up, slot) wherever no `with` or direct `eval` is in scope; the
bytecode compiler already knows each symbol and already keeps non-escaping locals on the stack. Environment records
become indexable slots; the dynamic path stays for `with`/`eval`. Reach: these cases to about 4-5x, and a broad gain
on real widgets. This is the largest single item.

### 2. The interpreter loop

Nova emits about as many instructions as QuickJS (15 for one iteration of `for (…) sum += i & 255`), but each cost
4.5 ns against 0.4 ns. Half of every profile was the dispatch itself: a call per instruction into a function that
was not inlined, a second one for anything not on a short "hot" list (comparisons, arithmetic, calls and returns
were not on it), and per-instruction bookkeeping.

Done: the dispatch is inlined into the loop (optimised builds only: unoptimised, the merged frame is too large), the
commonest instructions are dispatched directly, `ToNumeric` and conditional jumps skip conversions for values that
need none. Plain loops went from 11.1x to 7.5x, and a JavaScript call takes less native stack (1,683 nested calls
where there were 1,510).

Next: fused instructions for the commonest pairs (compare and branch, increment a stack slot, load a constant and
operate); constants and operands read from a slice the loop holds, not through the heap for each one. Reach: about
3-4x on plain loops.

### 3. Calls

A JavaScript call is 13-16x. Each one makes a new `Vm`, an execution context, a function environment record even
when nothing captures its variables, and binds its arguments through the general binding-pattern code. Recursion is
also shallower: about 2.7 KB of native stack per call level against 0.9 KB.

Next: no environment record for a function whose variables all live on the stack; parameters bound straight to
stack slots; the `Vm`'s stacks reused across calls. Reach: about 5-6x.

### 4. Properties

`o.x` is 8-20x: the access builds a Reference record and then reads through the inline cache, and the profile shows
generic own-property lookups next to it. Object creation is not the problem (literals 1.5x).

Next: one instruction for a named get and one for a named put that consult the cache directly, and a check that puts
hit it. Reach: about 4x.

### 5. Array and typed array elements

`a[i]` is 11-16x through the same Reference path with a generic property key; `typedArray.length` is a getter call
on every loop test.

Done: a typed array no longer converts a key such as `length` to a Number and back on every access to learn that it
is not an index (typed array loops 10.6x to 8.1x). Next: element get/put instructions with a short way for arrays
and typed arrays under an integer index; `length` of an array or typed array without a call. Reach: about 4x.

### 6. Numbers

Done: `Number::add`, `subtract` and `lessThan` walked the specification's case list (a dozen checks, each a match
over three representations); they are now the IEEE operation those steps describe, with a short way for two
integers, and comparisons of two Numbers no longer park the `Vm` for a collection that cannot happen.

Left: a double that does not fit the 56-bit inline form is allocated on the heap, so most float arithmetic
allocates (`math/` 7.8x, `typed/float64-math` 12x). The fix is a value layout with inline doubles, which touches
every `match` on `Value`: large, and after the items above.

### 7. Building strings

Every string is interned: creating one hashed all of it. Appending in a loop therefore hashed the whole string on
every step, and that, not the copy, was 91% of the time.

Done: the string table's hash is taken in 64-byte blocks, a long string made by a concatenation keeps the state of
its hash, and appending to it continues from there. 20,000 appends went from 185 ms to 22 ms (286x to 32x); building
character by character from 41 ms to 9 ms.

Left: each append still copies the string (now 70% of the time) and leaves the old one as garbage. Next: let a
string be a prefix view of a shared, append-only buffer, so that appending to the string that ends the buffer writes
only the new bytes. Reach: within 2-3x. Prepending (`s = x + s`, 38x) stays a copy in both engines; it is rare.

### 8. Other string work

`slice`/`substring`/`startsWith` are 6-7x and `charCodeAt` 6x: mostly items 1 and 2, plus interning each result.
Searching and splitting are level or faster.

### 9. Garbage

Allocation-heavy loops are 2-3x, strings as garbage 5x. Not a priority.

## Defects found on the way

- A loop of object destructurings in one call ran out of memory: each one scoped a key set that was released only
  when the function returned. Scoped collections are now released when the instruction ends. Fixed.
- `-1 * 0` was `+0`: the integer short way of `Number::multiply` returned an integer zero. Fixed.
- `String.prototype.repeat` goes through a lossy conversion, so a lone surrogate in the repeated string becomes
  U+FFFD (also listed open in `PLAN.md`). Not fixed.

## Checking a change

`cargo test -p nova_vm -p nova_embed -p nova_embed_ffi -p wtf8`, the widget sandbox's own suite against the built
library (224 tests in process, over the stream and hosted, the attack corpus, the compat corpus), then
`safe/bench/run.py --baseline …` for the numbers. test262 was not run for the first round.
