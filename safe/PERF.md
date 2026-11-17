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
| call | 14 | 10.4x | 4.0x |
| math | 4 | 9.7x | 3.8x |
| typed | 4 | 10.6x | 3.3x |
| prop | 13 | 7.1x | 3.3x |
| string | 26 | 10.6x | 3.0x |
| app | 8 | 6.1x | 2.9x |
| loop | 11 | 11.1x | 2.8x |
| array | 14 | 5.8x | 2.8x |
| map, set | 4 | 4.6x | 2.4x |
| regexp | 4 | 2.4x | 1.9x |
| gc | 3 of 5 | 3.6x | 1.6x |
| json | 2 | 1.0x | 1.0x |
| all | 108 | 7.6x | 2.9x |

The worst case was 303x (appending to a string 100,000 times); that one is 4.4x, and the worst is now prepending
(38x, area 7); after it come generators (7x), then calls that bind an arguments object or go through an accessor
(5-6x). 60 of the 108 cases are above 3x and 21 at or under 2x. One case ran Nova out of memory before
(`prop/destructuring`) and none does now. QuickJS runs out of
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
12.4x to 2.8x, loops from 11.1x to 4.2x, and everything else gained with them. A JavaScript call also takes less
native stack (1,683 nested calls where there were 1,510).

A collection is no longer looked for before every instruction either, only where one can have become due: when
code starts, after an instruction that is not plain, and on every turn of a loop.

The instruction pointer is a local of the loop across the plain instructions, and in the `Vm` around the others:
3-5% on loops and on calls.

The commonest pairs are one instruction now: `i++` and `i--` of a local whose value nothing uses (four
instructions before), a comparison with the jump on it, a local followed by a constant, two locals in a row. The
loop's own instructions of `for (var i = 0; i < n; i++)` are three where they were eight, and one iteration of
`for (…) sum += i & 255` is nine instructions where it was fifteen (QuickJS: eight). Each fused instruction does on integers what the plain ones did; anything else takes
the instructions it was fused from. Loops went from 3.9x to 2.9x (nested `while` loops 4.0x to 1.8x).

The executable, which every instruction that fetches a constant, a name, a cache or a binding site needs, is
looked up through its rooted handle once and again only after a collection has run (the heap counts them), not
by each of those instructions; and `this` of a call that keeps it in its execution context is read by the loop
itself. A property read and written in a loop went from 5.4x to 4.7x.

Tried and dropped: giving every plain instruction the length of its own to advance by, so that where the next
instruction is would not wait for the table of lengths. It made loops 13-32% slower; the table is not what the
dispatch waits for.

Left: about 1 ns an instruction, of which the dispatch is still about half (a quarter of the time of a loop of
plain calls is the twenty machine instructions between two bytecode instructions: the bounds check, the argument
count through a table, the jump table). Next: more of the same fusing (a binary operator with the store into a
local after it; an operator with a constant operand), fewer
instructions for the same code, by fusing the commonest pairs (compare and branch, increment a stack slot, load a
constant and operate), which is where QuickJS's 8 instructions for the same iteration come from. Reach: about
2-2.5x on plain loops.

### 2. Variables that are not locals of the running function

Every read or write of a closure variable, a module-level variable or a global resolved the name at run time:
`ResolveBinding` walks the environment chain and looks the name up in each record, builds a Reference, and
`GetValue`/`PutValue` goes through it. Widget code is closures over a module scope, so this is in every case that
is not a bare loop. QuickJS resolves these when it compiles, to an index.

Done, for variables of enclosing functions and blocks. Each place in the code that names such a variable remembers
where the name was found (a `BindingSite`: so many environments out, at such a slot), and reads and writes the
variable there with one instruction and no Reference. That is sound wherever no `with` and no direct `eval` is in
any scope between the name and its declaration, which the compiler knows; the remembered place is still checked
when it is used (the kinds of the records on the way, the name at the slot), and anything unusual (a variable not
yet initialized, a constant written to) takes the general way and gives the error that way gives. A loop over a
closure variable went from 18x to 4.9x, and calls, property and element access gained 10-30% with it, since the
functions and objects they use are closure variables.

Remembering the place alone, with the Reference kept, gained 3-8%: the lookup was the small part and the
Reference the large one.

Done for the names of the global scope too, in a Script: a `var`, a function, a `let`, `const` or `class` of the
top level, a built-in such as `Math` or `undefined`. The place remembered is a slot among the lexical declarations
of the global scope, or a slot among the properties of the global object together with how many lexical
declarations there were, since a later one of the same name comes first. The global object's shape is its own and
changes in place, so what is checked at every use is the name at the slot, not the shape. An assignment keeps the
specification's order: `name = value` resolves the name before the value is evaluated (a name not bound then
throws in strict code even when evaluating the value binds it), and a compound assignment, which reads first,
does not ask the global object for the name once more than the specification does. A loop over a global went
from 9.1x to 5.2x, and whatever names a built-in gained with it (`math/` 5.5x to 4.2x, a call of `Math.max` 8.3x
to 5.4x).

Left: a read of a global is about 8 ns against 3 for a closure variable: the way from the running realm to the
slot is a dozen loads (the realm, the global record, its two halves, the object, its shape, the keys, the
values). `typeof name` still goes through a Reference. The code of a Module and of an `eval` is compiled as
before, and module variables are resolved the long way.

### 3. Calls

A JavaScript call was 13-20x. Each one made a new `Vm`, an execution context and a function environment record,
bound its parameters through the general binding-pattern code (an iterator over the arguments, five instructions
and more for two parameters), made a second environment record for the lexical declarations of a function that is
not strict, and left through the cold path for unusual results.

Done: one instruction binds the parameters of a function whose parameters are all plain names in stack slots; a
function that is not strict and holds no direct `eval` makes no separate record for its lexical declarations (that
record is only there for an `eval` to look in); `Return` ends the loop directly. And a function that is not
strict is called without a Function Environment Record where a strict one is (nothing in it can observe the
record): what OrdinaryCallBindThis makes of the this argument (the global this value for undefined and null, an
object of a primitive) is made when the call is prepared and kept in the execution context.

And a function whose parameters are all plain names in stack slots starts with its arguments there: the call
copied them three times before (out of the caller's stack, into a list rooted for the collector, and from there
one by one into the slots, with an instruction to make each slot first). `function add(a, b) { return a + b; }`
was eight instructions and is three. A plain call is 4.4x, a callback 3.5x.

A call whose code pushes a Reference (a method that assigns to `this.x`) or enters a loop or a `try` allocated
that stack and freed it when it returned; the emptied stacks are kept, and a `Vm` takes one when it first pushes
to it. (Pooling them with the two stacks every call takes made a plain call 7% slower.) A method call went from
5.7x to 4.3x, calls to 4.0x.

Left: what is left is spread thin, with no single cost much above a tenth: the caller's `Vm` moved aside and
back for the collector around every call, the callee's made and recycled, the execution context, the copy of
the arguments out of the caller's stack; under all of it the dispatch of area 1, a quarter of the time.
Functions with a rest parameter, defaults or an arguments object still bind the old way (5-6x), and generators
are 8x. Recursion is also shallower: about 2.7 KB of native stack per call level against 0.9 KB. Next: keep one
`Vm` across calls. Reach: about 3x.

### 4. Properties

`o.x` was 8-23x: the access built a Reference record and went through the general `[[Get]]` to reach the property
cache, and `o.x = v` did not use the cache at all: it ran the whole of OrdinarySet, with two searches of the keys.

Done: where the property cache knows the shape of an ordinary object, `o.x` is read from the offset the cache gives
(the object's own, or a prototype's, or known absent), and `o.x = v` writes an own writable data property there.
Anything else (an accessor, a property that is not writable, a shape the cache does not know yet, an object that is
not ordinary) takes the general way. One shape is 5.9x, four shapes 4.2x.

Left: accessors are calls (`prop/getter-setter` 8x), and a method call (`prop/prototype-chain-method` 7x) is a
call. Functions, Arrays and other objects with a backing object do not take the short way for their named
properties yet.

### 5. Array and typed array elements

`a[i]` was 11-19x through the same Reference path with a generic property key.

Done: `a[i]` of an Array with an element there is read from the element storage, `a[i] = v` writes an element that
is there as a plain value, and `a.length` is the length (5.0x for reads and writes in a loop, 4.3x for a grid). A
hole, an index outside the Array, an accessor or an element that is not writable takes the general way. A typed
array no longer converts a key such as `length` to a Number and back on every access to learn that it is not an
index.

Done for typed arrays as well: with an integer index an element is read with TypedArrayGetElement and written
with TypedArraySetElement straight from the loop (a value that is not a Number yet is converted the general way,
since that can run a script), and `a.length` is what the accessor of %TypedArray%.prototype returns, without the
call, where the property cache finds that accessor for the TypedArray's shape. Typed arrays went from 5.5x to
4.1x, a loop that fills and sums a Uint8Array from 6.1x to 3.8x.

An element is also read and assigned with one instruction each now, and no Reference: a read for its value
with the object and the key where the expression left them, and `a[i] = v` whose own value nothing uses with the
object and the key kept on the stack while the value is evaluated (the order of evaluation is the Reference
way's). Arrays went from 3.5x to 3.0x.

`a.push(x)` was two slow things: the lookup of `push` went the general way (only ordinary objects read through
the property cache from the loop), and the built-in ran `Set` in full for the item and for the length. A named
property of an Array is now read through the cache as `[[Get]]` reads it, and `push` appends directly where its
steps come to that: the Array can grow and has no element descriptors, and its prototype chain is the usual one
with no property under an integer key on it, which is looked at on every call, with nothing remembered that a
change to a prototype would have to undo. `array/push-then-pop` went from 6.6x to 3.9x, the canvas command
buffer from 8.9x to 3.9x, arrays to 2.8x and the app cases from 3.8x to 3.0x.

Left: a compound assignment to an element (`a[i] += 1`) and one whose value is used still make a Reference.
Other built-ins that take a callback or build an Array (`map`, `filter`, `forEach`) run their general steps.
Float64 elements allocate (area 6).

### 5a. Iteration

`for (var ch of string)` was 10x: every step called %StringIteratorPrototype%.next, which made an iterator result
object (through two vectors allocated and freed for its two entries, a third of the time), and the loop then read
`done` and `value` back with two general property lookups. A loop over a generator, a Map or any other iterator
did the same reads.

Done: an object made from a list of entries gathers their values on the stack; a String Iterator whose `next` is
the original one is stepped directly, as an Array Iterator already was; and the `done` and `value` of a result
that is an ordinary object with those as plain properties of its own are read from their slots. Iterating a
string by code point is 3.7x, a Map 2.7x, a generator 7.0x (8.2x).

Left: a generator makes its result object in its own bytecode at every `yield`, the loop still calls `next`
through the built-in, and the generator's `Vm` and execution context are moved in and out of the generator at
every step. Stepping a generator directly, with the yielded value handed over without an object, is next. (A
suspended `Vm` keeps its stacks as vectors now instead of cutting them to size at every `yield`; that removed
two reallocations a step and changed no measured time.)

### 6. Numbers

Done: `Number::add`, `subtract` and `lessThan` walked the specification's case list (a dozen checks, each a match
over three representations); they are now the IEEE operation those steps describe, with a short way for two
integers, and comparisons of two Numbers no longer park the `Vm` for a collection that cannot happen. Two integers
whose result is an integer or a Boolean are now handled in the loop itself (area 1).

Left: a double that does not fit the 56-bit inline form is allocated on the heap, so most float arithmetic
allocates (`math/` 4.2x, `typed/float64-math` 5.2x). The fix is a value layout with inline doubles, which touches
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
- `typeof x` before the `let x` of the same function panicked (the engine died): the compiler asked for a
  Reference that a stack variable does not have. It throws a ReferenceError now. Fixed.
- `with (object) { … }` evaluates the object and runs the body without an Object Environment Record, so the
  body never sees the object's properties (the bytecode compiler stops at step 2 of the statement's evaluation).
  Sloppy code only. Not fixed.
- The `arguments` object of a function that is not strict is not tied to its parameters (`arguments[0] = 1` does
  not change the first parameter): the engine always makes the unmapped kind. Upstream, and expected to fail in
  test262. Not fixed.
- What `nova_engine_memory_used` reports between two collections does not count the property storage of the
  objects made since the last one (20,000 objects of two properties: 320 KB counted, 1 MB found by the
  collection). Calls used to hide this with the environment records they allocated; a test that leaned on
  that now measures at collections. The limit holds as before: a script that keeps objects of 200 properties
  under an 8 MB limit, in a loop or through calls, ends at the same point with and without this round's
  changes. Not fixed here (the allocator's own count on `android-dev` replaces the estimate).

## Checking a change

`cargo test -p nova_vm -p nova_embed -p nova_embed_ffi -p wtf8`, the widget sandbox's own suite against the built
library (224 tests in process, over the stream and hosted, the attack corpus, the compat corpus), then
`safe/bench/run.py --baseline …` for the numbers, and test262 as `safe/tools/gate.sh` runs it (no unexpected
result; 13 or 14 tests that timed out now pass, depending on the run).
