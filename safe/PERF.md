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

Where the time goes inside the interpreter loop does not show in a profile by function: the loop is one function.
`xctrace record --template 'Time Profiler'` and `xctrace export` give the sampled addresses, and those against the
disassembly give the time per machine instruction. That is how the loop's own share was found (area 1).

## Where it stands

Nova's time over QuickJS's, geometric mean per group, on an Apple M-series machine (QuickJS fork `fbc91d8`, `-O2`;
Nova release with fat LTO, rustc 1.96.1). "Before" is `c13a0dfc` with the embedding fixes; "now" has the changes
below.

| Group | Cases | Before | Now |
|---|---|---|---|
| call | 14 | 10.4x | 7.7x |
| typed | 4 | 10.6x | 6.7x |
| math | 4 | 9.7x | 5.9x |
| prop | 13 | 7.1x | 5.1x |
| app | 8 | 6.1x | 5.0x |
| array | 14 | 5.8x | 4.6x |
| loop | 11 | 11.1x | 4.4x |
| string | 26 | 10.6x | 3.9x |
| map, set | 4 | 4.6x | 3.1x |
| regexp | 4 | 2.4x | 2.2x |
| gc | 3 of 5 | 3.6x | 2.1x |
| json | 2 | 1.0x | 1.0x |
| all | 108 | 7.6x | 4.5x |

The worst case was 303x (appending to a string 100,000 times); that one is 4.2x, and the worst is now prepending
(38x, area 7). One case ran Nova out of memory before (`prop/destructuring`) and none does now. QuickJS runs out of
its 8 MB on two cases Nova fits (30,000 retained objects, 50,000 closures): Nova's heap is the more compact one.

What is written in Rust or C on both sides is level: JSON, RegExp, `indexOf`, `split`/`join`, `sort`. The gap is in
running JavaScript itself.

## The areas, and how far each can go

Reach figures are estimates from the profiles, not measurements.

### 1. The interpreter loop

Nova emits about as many instructions as QuickJS (15 for one iteration of `for (…) sum += i & 255`), but each cost
4.5 ns against 0.4 ns, and this is under every other number here. Sampled by machine instruction, 68% of a plain
loop was not the instructions at all but what the loop did around each one: decoding both arguments whether or not
the instruction has them, counting down to the interrupt poll, checking for a collection and for the debug print,
looking at the result, and releasing what the instruction scoped.

Done, in two rounds. First: the dispatch is inlined into the loop (optimised builds only: unoptimised, the merged
frame is too large) and the commonest instructions are dispatched directly. Second: the instructions that cannot
fail, run a script or scope a value (stack moves, constants, jumps, strict equality, and the comparisons and
arithmetic of two integers) go straight to the next instruction, with no result to look at and nothing to release;
arguments are read as four bytes of which an instruction uses what it has; the interrupt countdown is taken at
backward jumps and at calls (8 steps each) and no longer at every instruction. A plain counting loop went from
12.4x to 3.0x, loops from 11.1x to 4.4x, and everything else gained with them. A JavaScript call also takes less
native stack (1,683 nested calls where there were 1,510).

Left: 1.3 ns an instruction, of which the dispatch is still 60% (the instruction pointer goes through memory, the
argument count through a table, and a collection is checked for before each instruction). Next: fewer
instructions for the same code, by fusing the commonest pairs (compare and branch, increment a stack slot, load a
constant and operate), which is where QuickJS's 8 instructions for the same iteration come from. Reach: about
2-2.5x on plain loops.

### 2. Variables that are not locals of the running function

Every read or write of a closure variable, a module-level variable or a global resolves the name at run time:
`ResolveBinding` walks the environment chain, looks the name up in each record, builds a Reference, and
`GetValue`/`PutValue` goes through it. A loop over a closure variable is 8.2x and over a global 9.3x (they were
18x and 17x), and widget code is closures over a module scope. QuickJS resolves these when it compiles, to an
index.

Measured: remembering, per place in the code, where the name was found (environments outwards, slot) and checking
that with one comparison is sound wherever no `with` or direct `eval` is in scope, and it gains only 3-8% on a
closure variable one level out. The lookup is the small part; the Reference built for every access and the three
general instructions it passes through are the large one. The change is kept aside (`git stash`, "binding sites")
until it carries its weight.

Next: on top of the remembered place, one instruction that reads the variable into the result and one that writes
it, with no Reference; then the same for globals through the existing property cache. Reach: about 4-5x for these
cases.

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
integers, and comparisons of two Numbers no longer park the `Vm` for a collection that cannot happen. Two integers
whose result is an integer or a Boolean are now handled in the loop itself (area 1).

Left: a double that does not fit the 56-bit inline form is allocated on the heap, so most float arithmetic
allocates (`math/` 5.9x, `typed/float64-math` 10x). The fix is a value layout with inline doubles, which touches
every `match` on `Value`: large, and after the items above.

### 7. Building strings

Every string is interned: creating one hashed all of it. Appending in a loop therefore hashed the whole string on
every step, and then copied it: quadratic twice over. 160,000 appends took 12 s against 5 ms.

Done. The string table's hash is taken in 64-byte blocks, and a long string made by a concatenation keeps the
state of its hash, so appending continues from there. And an appended string is kept as shared pieces (the text
before, and what was added) that are joined only when something reads the text; appending to it shares all of the
pieces and adds one. The appended bytes are counted against the heap limit when they are added, a join is counted
when it happens, and a join that would take the heap past the limit ends the script as any allocation there does.
Appending is linear: 160,000 appends take 24 ms, and the long append cases are 2.5-4.2x (they were 170-300x).

Left: prepending (`s = x + s`, 38x) still copies and hashes the whole string every time. It is rare.

### 8. Other string work

`slice`/`substring`/`startsWith` and `charCodeAt` are 4-6x: mostly areas 1 and 2, plus interning each result.
Searching and splitting are level or faster.

### 9. Garbage

Allocation-heavy loops are about 2x, strings as garbage 5x. Not a priority.

## Defects found on the way

- A loop of object destructurings in one call ran out of memory: each one scoped a key set that was released only
  when the function returned. Scoped collections are now released when the instruction ends. Fixed.
- `-1 * 0` was `+0`: the integer short way of `Number::multiply` returned an integer zero. Fixed.
- `String.prototype.repeat` goes through a lossy conversion, so a lone surrogate in the repeated string becomes
  U+FFFD (also listed open in `PLAN.md`). Not fixed.
- `with (object) { … }` evaluates the object and runs the body without an Object Environment Record, so the
  body never sees the object's properties (the bytecode compiler stops at step 2 of the statement's evaluation).
  Sloppy code only. Not fixed.

## Checking a change

`cargo test -p nova_vm -p nova_embed -p nova_embed_ffi -p wtf8`, the widget sandbox's own suite against the built
library (224 tests in process, over the stream and hosted, the attack corpus, the compat corpus), then
`safe/bench/run.py --baseline …` for the numbers. test262 has not been run for these changes.
