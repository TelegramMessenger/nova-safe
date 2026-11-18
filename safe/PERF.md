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
| call | 14 | 10.4x | 3.2x |
| typed | 4 | 10.6x | 3.0x |
| math | 4 | 9.7x | 2.8x |
| prop | 13 | 7.1x | 2.7x |
| array | 14 | 5.8x | 2.6x |
| string | 26 | 10.6x | 2.5x |
| app | 8 | 6.1x | 2.5x |
| map, set | 4 | 4.6x | 2.3x |
| loop | 11 | 11.1x | 1.9x |
| regexp | 4 | 2.4x | 1.9x |
| gc | 3 of 5 | 3.6x | 1.5x |
| json | 2 | 1.0x | 0.9x |
| all | 108 | 7.6x | 2.5x |

The worst case was 303x (appending to a string 100,000 times); that one is 4.1x, and the worst is now prepending
(38x, area 7); after it come generators (6.3x), then calls that bind an arguments object or go through an accessor
(5-6x). 46 of the 108 cases are above 3x and 31 at or under 2x. One case ran Nova out of memory before
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

A `switch` tested its cases one after another, three instructions a case, the comparison among them on the
slow path, and kept a completion value that a function body never returns. A `switch` whose selectors are all
literals (Numbers, a minus sign allowed, Strings, Booleans, null) is one instruction now: `JumpToCase` looks the
value up in a table kept with the executable, by index where the selectors are integers close together and by
comparing in order otherwise, which is what the tests did, since literals have no effect and the first equal
one wins. Selectors that are not literals keep their tests. In a function body the cases keep no completion
value. A `switch` of eight cases in a loop went from 5.0x to 1.8x.

`===`, `!==`, `==` and `!=` before a jump are one instruction with it, as `<` already was; two integers are
compared in place and two values of one type are loosely equal where they are strictly equal, so only operands
of different types take the general way. `x === 3` on its own no longer leaves the loop of plain
instructions, and neither do the instructions that only move a value or test it for null and undefined, or
the ones that enter and leave the handler every loop pushes. A loop of strict comparisons went from 3.9x to
2.5x, testing each character of a string against five literals from 1.5x to 1.1x.

Tried and dropped: giving every plain instruction the length of its own to advance by, so that where the next
instruction is would not wait for the table of lengths. It made loops 13-32% slower; the table is not what the
dispatch waits for.

Counted over the cases that are written like application code, one instruction in five was the skeleton of a
`for` loop: the jump back, the increment, the two operands of the test, the test with its jump out. A test of a
local against a constant or another local, with the jump on it, is one instruction now (twelve of them, for the
four orderings, `===` and `!==`, each with a constant or a local): these have three operands, a stack slot, a
constant or slot and the jump target, in eight bytes where every other instruction has at most four, and their
handlers read the target from the instruction stream; nothing changes in how the others are decoded. A `for`
loop whose completion value nothing sees, with no binding made per turn, and whose test is a comparison of
names, literals, `this` and properties named after a dot, has that test at its end, compiled a second time
before the loop for whether it is entered: what runs, and in what order, is the same, and a turn no longer
jumps forward to the test. There the increment, the test and the jump back are one instruction where the test
is of the counter against a constant or a local (`IncrementIndexJumpIfLessThanConstant` and fifteen like it),
which makes the skeleton of `for (var i = 0; i < n; i++)` one instruction a turn where it was four; with a
bound read from a property (`i < a.length`) it is four where it was five. Only such tests are compiled twice:
none makes a function, a class or a template object, whose identity two places would not share.

The jump back of such a loop is the test itself, and the host is asked whether to interrupt on jumps back.
Where the operands of the test are not integers the instruction takes the general way, which asked nothing:
a loop over doubles or strings could not be interrupted. Any instruction that leaves the general way with
the instruction pointer moved back now asks. Found by the checks of this change before it was committed.

A loop that sums integers went from 2.4x to 1.9x, an empty one from 2.3x to 2.0x, a loop over a closure
variable from 3.3x to 3.0x; the group from 2.5x to 2.2x.

Counted over one run of every case (286 million instructions), one instruction in seven was `Load`, which
only pushes the result so that the next instructions can make another. Four sequences around it take their
operands from where they are instead:

- an arithmetic or bitwise operator whose right operand is a constant, with the left one as the result
  (`ApplyBinaryOperatorWithConstant`: `x & 255` was three instructions) or in a local
  (`ApplyBinaryOperatorToIndexWithConstant`: `i - 1` was two). One instruction serves the eleven operators:
  which one is an argument, in four bits beside the stack slot, so both are decoded like any other;
- `a[i]` where the key is a local and the base is the result (`GetValueWithIndexKey`, three before);
- a variable of an enclosing function pushed (`LoadBindingAtSite`, two before);
- a constant pushed right after another push, which the instruction that pushes a constant does alone.

Where the compiler wants the unpushed form later (a comparison with a constant, an operator), an instruction
that ends in a push is made the one that does not. One turn of `a[i] = (a[i - 1] + a[i]) & 1023` on an array of
the enclosing function is 16 instructions where it was 23: 3.8x to 3.1x. The run of every case is 241 million
instructions now, 16% fewer, and `Load` is one in eleven. Reading and writing a grid went from
2.7x to 2.1x, the RGBA loop over a typed array from 4.2x to 3.5x, a `switch` in a loop from 1.4x to 1.2x.

What the step between two instructions costs was looked at again on that loop: 31 machine instructions, 40% of
its samples, with the bytecode's address and length read from the stack twice each. Not changed in this round;
fewer instructions a turn was the surer gain. What the same count says to fuse next: an operator and the
store of its result in a local (`ApplyAdditionBinaryOperator` then `PutValueToIndex` is 3.6% of what runs),
the push of what an operator with a constant made (2.9%), the function of a method call and its Reference
(2.0%), the last argument and the call (2.4%).

Left: about 1 ns an instruction, of which the dispatch is still about half (a quarter of the time of a loop of
plain calls is the twenty machine instructions between two bytecode instructions: the bounds check, the argument
count through a table, the jump table). The counts of pairs say what to fuse next: a value pushed and then a
constant operand and its operator (`x & 7`: three instructions), an operator and the store into a local after
it, a local and its property (which needs the three operands the wide instructions now have), `while` loops
with the test at the end. Reach: about 2x on plain loops.

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

A call of a built-in function went the same way as any call: the arguments copied to a list, the caller's `Vm`
moved aside, an execution context pushed, the function found through three dispatches: for `Math.floor(x)`
most of the time of the call. The functions of Math that only compute a Number from Numbers
(`floor`, `ceil`, `round`, `trunc`, `abs`, `sqrt`, `sign`, and `max` and `min` of integers) are marked when
their function objects are made, and the call instruction runs a marked function in place when its arguments
are Numbers: the same code the function runs after converting its argument, which takes no collector scope
and so cannot collect, throw or call anything. Any other argument, and a function put in their place, takes
the call. `Math.floor(Math.sqrt(i)) + Math.max(i & 7, 3)` went from 4.8x to 2.9x, rounding and clamping in a
loop from 4.4x to 2.8x. What is left of those is reading `Math` and the function off it (areas 2 and 4).
`charCodeAt` of a String value at an integer is run in place the same way; a function that takes its this
value has it from the Reference the call is made through. So are `sin`, `cos` and `pow` of Numbers, and
`max` and `min` of any Numbers: those two first try the arguments as Numbers, by one function that the call
and the instruction share, so that zeros and NaN come out the same either way. Trigonometry in a loop went
from 3.85x to 3.53x.

A call of a function still went through half a dozen native frames between the caller's loop of instructions
and the callee's, each saving and restoring its registers, with the caller's `Vm` moved aside for the collector
and a new one made for the callee. A `Vm` now runs the function itself, in the same loop, where the function
is an ordinary one (not a generator, not async, not a class constructor), keeps its parameters in stack slots
and needs no Function Environment Record: the call instruction puts the caller's stacks, its place and its
code aside in a frame, pushes the callee's execution context as \[\[Call]] does, and goes on in the callee's
code; `Return` takes the caller's state back, and an error that the callee does not handle goes to the
caller's handlers, frame by frame. Everything else about the call is as it was: the execution context, the
this value, the count of calls that ends recursion, the interrupt poll and the collection check on entry.
The frames are slots of one stack that the Agent keeps: entering and leaving swap the stacks in a slot with
the `Vm`'s, and the collector marks the slots in use. A call that is not run in place (from a built-in, of a
bound function, a generator, a constructor) takes the native way as before, and its code runs its own calls
in place.

A plain call went from 4.5x to 3.6x, a method call from 4.2x to 3.6x, recursion from 4.5x to 3.4x, a call
through the prototype chain from 4.3x to 3.5x; a callback of a built-in, which is called the native way, is
1 to 5% slower than it was.
Recursion of such functions no longer takes native stack: it ends at the 3,500th call wherever the native
stack would have run out first (after 2,325 calls in the command-line build).

Three things that cost more than the change saved until they were found. With the frame switch written
inside the loop of instructions, the compiler kept the instruction pointer in memory, and the gain was 8%
with every other kind of call 10-18% slower: the switch is now in a loop around that one, which is as it
was. A `Vm` with the frames inside it was 32 bytes larger and no longer dropped in line: every native call
paid for both (the frames are the Agent's for that reason). And the function that prepares the execution
context, once shared by the two ways, stopped being inlined into the native one.

Left: per call in place, the execution context (64 bytes pushed and removed), the frame, a rooted handle of
the callee's executable, a count on its instructions; under them the dispatch of area 1. A stack shared by
caller and callee, with the arguments left where they are, would take most of the frame away; it changes
how every instruction finds a stack slot. Functions with a rest parameter, defaults or an arguments object
still bind the old way and are called the native way (5-6x), and generators are 6.6x. Next: those, and `new`
of a plain constructor, in place. Reach: about 2.5x.

### 4. Properties

`o.x` was 8-23x: the access built a Reference record and went through the general `[[Get]]` to reach the property
cache, and `o.x = v` did not use the cache at all: it ran the whole of OrdinarySet, with two searches of the keys.

Done: where the property cache knows the shape of an ordinary object, `o.x` is read from the offset the cache gives
(the object's own, or a prototype's, or known absent), and `o.x = v` writes an own writable data property there.
Anything else (an accessor, a property that is not writable, a shape the cache does not know yet, an object that is
not ordinary) takes the general way. One shape is 5.9x, four shapes 4.2x.

`o.x = v` whose value nothing uses made a Reference of `o` and `x`, pushed it on the Reference stack while `v`
was evaluated, and took it back to assign. It keeps `o` on the value stack instead, and one instruction assigns
by the cache (`PutPropertyWithCache`); the general way makes the Reference when it is needed, from the same
object, after the value, as before. And `this.x` is one instruction (`GetThisPropertyWithCache`) where the call
keeps its this value itself, two otherwise. One shape went from 4.8x to 4.2x.

A property of a local (`p.x`, `a.length`, the function of `p.m()`) was two instructions, the local and the
read; it is one, with the stack slot as a third operand after the identifier and the cache. This is what
most property reads in application code look like: the particle update went from 3.95x to 3.67x.

An assignment by the cache looked the storage up twice, once with its descriptors (a hash lookup for whether
the property is writable) and once to write: it is one look now, which asks for a descriptor only where the
storage's arena has any. This changed little (2% on one shape).

`o.x = v` that adds the property (every assignment in a constructor, and `o.a = 1; o.b = 2` after `{}`) took
the general way each time: the cache had nothing for a shape without the property, because only reads record
that a property is absent. The assignment's general way now looks the property up as a read would before it
assigns, which records the absence for that shape; the next assignment on that shape finds it, and where the
object is ordinary and extensible, appends the value and records the new shape's offset
(`ordinary_add_own_data_by_cache`). An absence is recorded only by a lookup that walked the whole chain through
ordinary lookups and found no property of that name, so there is no setter and nothing that is not writable to
find, and it is dropped when an object of the chain gains the property or gets another prototype: the append
is what OrdinarySet would do. A Proxy in the chain records nothing, and neither does a typed array for the
names it answers itself (`NaN`, `Infinity`). Adding six properties one by one went from 3.6x to 1.3x, `new` of
a class from 4.8x to 3.2x and a constructor that calls `super` from 3.9x to 3.3x.

This leans on the caches being told of every change to a prototype, and checking that found a way they were
not (`Object.create`, under Defects): fixed in the same round, and tested for every way an object gets its
prototype.

Left: a property read is the cache (up to four shapes compared), then the object's record, its shape's record
for the capacity of the storage, a jump on that capacity to the arena of that size, and the slot: about eight
dependent loads where QuickJS has three. The loop of `o.x = o.x + o.y` is 4.2x with dispatch a third of it.
Accessors are calls made the native way (`prop/getter-setter` 5.0x). Functions, Arrays and other objects with
a backing object do not take the short way for their named properties yet.

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

Any two Numbers stay in the loop of plain instructions now, not only two integers: the arithmetic and bitwise
operators (also with a constant, area 1), the comparisons, and the tests of loops with their jumps. An
operator on doubles took the general way for every operand that was not an integer, and so did a loop whose
bound or counter was one. The result of an operator may be a Number of the heap, which is the one thing a
plain instruction allocates: no collection can be due before the loop looks for one again, at its next turn
or at the next instruction that is not plain. A hash with a multiplication that leaves the safe integers
went from 1.7x to 1.2x, a loop of float arithmetic from 3.7x to 3.1x, integers mixed with doubles from 2.4x to
1.9x, float arithmetic over a Float64Array from 4.8x to 4.1x.

Left: a double that does not fit the 56-bit inline form is allocated on the heap, so most float arithmetic
allocates (`math/` 2.8x, `typed/float64-math` 4.1x). The fix is a value layout with inline doubles, which touches
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

`slice`/`substring`/`startsWith` and `charCodeAt` were 4-6x. A profile of a tokenizer showed about a sixth of its
time in finding the method: a property of a String value went the general way every time (a Reference, the wrapper
that a primitive is given, the search of %String.prototype%), the property cache never asked. Now `s.length` is
read from the String, and any other name is looked up as \[\[Get]] of the %String.prototype% of the running
realm leaves it in the cache, under that object's shape: its own property or one further up, or known absent;
an accessor there, and a name the cache has not seen, take the general way. The same for a Number value and
%Number.prototype% (`n.toFixed(2)`). `charCodeAt` in a loop went from 3.3x to 1.8x, slicing from 4.7x to 3.3x,
case conversion and trimming from 1.7x to 1.3x, the tokenizer from 3.9x to 2.5x; the group from 2.9x to 2.5x.

Left: the calls themselves, which are native ones (area 3), and interning each result. Searching and splitting
are level or faster.

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

- An object that became a prototype through `Object.create`, or as the `prototype` of a subclass of Array, of a
  typed array or of Function, or through `Reflect.construct` of those, was not known to the property caches as
  a prototype. After one read through it, what was added to it, removed from it or redefined on it, and a change
  of its own prototype, went unseen: `var p = {}, o = Object.create(p); o.x; p.x = 1; o.x` gave undefined, and
  `var q = { y: 1 }, r = Object.create(q); r.y; delete q.y; r.y` panicked (an index out of bounds; the engine
  died). Upstream; test262 has no test that fails for it. The caches' invalidation is keyed on a prototype's
  shape being intrinsic, which `new`, `setPrototypeOf` and `__proto__` in a literal saw to and the creation of
  an object by its prototype alone did not. Now the first shape made for a prototype makes the prototype's own
  shape intrinsic (`ObjectShape::get_shape_for_prototype`). Fixed.
- ToInt32 and its like went through a 64-bit integer, which stops at its largest value: every Number from 2^63
  up gave the same bits (`1e21 | 0` was -1, `new Int32Array([1e21])[0]` too), for the bitwise operators, the
  elements of typed arrays and the setters of a DataView. Upstream; test262 has no test that fails for it.
  Fixed (`truncate_modulo_2_64`).
- `%` of two Numbers was computed from the quotient rounded to a double, and was wrong wherever that rounding
  reaches a whole number: `1e21 % 3` was 0, and so were `5.5 % 1.1` (1.0999999999999996) and `10 % 0.1`
  (0.09999999999999945). Upstream; test262 has no test that fails for it. It is the remainder of the two
  doubles now, which is exact. Fixed.
- A declaration with an initializer (`var x = 1;`, `let`, `const`, a class) takes the place of the completion
  value before it: `eval("5; var x = 1;")` gives undefined where it must give 5, and after a class
  declaration it gives the class. Only the value of a script or of `eval` sees it. Upstream. Not fixed.
- Whether an object is extensible is a bit in its reference to its shape, and three places that replace the
  shape dropped it. An object that was frozen, sealed or made non-extensible took new properties again (and
  `Object.isFrozen` said false) once it became a prototype, by `new`, `setPrototypeOf`, `__proto__` or class
  heritage; after one of its properties was deleted; and when private fields were added onto it. Upstream. One
  staging test of test262 failed for it (`staging/sm/Symbol/property-reflection.js`) and passes now. Fixed.
- `Object.prototype.__proto__` does not exist: `o.__proto__` reads undefined and `o.__proto__ = p` adds an own
  property named `__proto__` (the `__proto__: p` of an object literal does set the prototype). Upstream, with
  the rest of Annex B's additions to Object.prototype. Not fixed.
- A `let` or `const` of one `case` read in another case that is entered first (`switch (2) { case 1: let x;
  case 2: x; }`) gives undefined where it must throw a ReferenceError: the variable is a stack slot and the
  compiler takes it for initialized. Upstream. Not fixed.
- Two cases whose time is in native code (`gc/string-garbage`, which hashes every string it makes, and
  `array/index-of-includes`) read 7-10% slower in the build of the loop changes than in the build just before
  it, whose code for them is the same. Taken for the placement of code by the linker; not explained further.
- The build with the fix for prototypes made by `Object.create` reads 5-7% slower on three cases that never
  come near it (`array/index-read-write` 13.7 to 14.4 ms, `app/particles-step` 5.8 to 6.1 ms,
  `array/two-dimensional-grid` 8.5 to 8.9 ms), reproducibly, against the same code without the fix built side
  by side.
  The two binaries spend their samples in the same functions, and the instructions of the dispatch in `Vm::run`
  are the same one for one: the function starts 100 bytes later, and that is all that differs. Moving the fix's
  rare path out of line changed nothing. It is the placement of the loop, as with the two cases above; over
  all cases the build is 1% faster than the one before the round. Not pursued: making the placement of
  `Vm::run` the same in every build (an alignment for functions) is the thing to try.
- The same was seen in the round after: `string/index-of` went from 2.2 to 2.9 ms (0.5x to 0.7x) with not a
  line of the search changed. `Searcher::find_from` has the same instructions in both builds, 0x6d8 bytes
  apart. A case whose time is one small loop can move by a third on where the linker puts it.
- One run of `limits_tests` in fourteen had one failing test (which one was not captured); the thirteen other
  runs, eight of them of that file alone, passed. Its tests end scripts by wall-clock deadlines. Not explained.

## Checking a change

`cargo test -p nova_vm -p nova_embed -p nova_embed_ffi -p wtf8`, the widget sandbox's own suite against the built
library (224 tests in process, over the stream and hosted, the attack corpus, the compat corpus), then
`safe/bench/run.py --baseline …` for the numbers, and test262 as `safe/tools/gate.sh` runs it (no unexpected
result; 13 or 14 tests that timed out now pass, depending on the run).
