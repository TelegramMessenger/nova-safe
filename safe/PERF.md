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
| typed | 4 | 10.6x | 2.3x |
| call | 14 | 10.4x | 1.9x |
| math | 4 | 9.7x | 1.9x |
| string | 26 | 10.6x | 1.4x |
| prop | 13 | 7.1x | 1.5x |
| map, set | 4 | 4.6x | 1.6x |
| app | 8 | 6.1x | 1.7x |
| array | 14 | 5.8x | 1.5x |
| regexp | 4 | 2.4x | 1.7x |
| loop | 11 | 11.1x | 1.3x |
| gc | 3 of 5 | 3.6x | 0.9x |
| json | 2 | 1.0x | 0.9x |
| all | 108 | 7.6x | 1.5x |

The worst case was 303x (appending to a string 100,000 times); that one is 1.9x, and so is prepending, which
was the worst after it (38x, area 7). The worst are now arithmetic on the elements of a `Float64Array`
(3.0x), a step of a particle animation (2.9x), the call of a closure (2.8x) and the properties of one
object read and written in a loop (2.6x). 81 of the 108 cases are at or under 2x (80 or 81 from one
run to the next: a few sit within a few hundredths of 2x and cross it), 16 of them at or under
QuickJS's time. Over all cases it is 1.53x: 2.12x before the round of patterns, Array iteration and
array literals, 2.05x after it, 2.03x after the round of `for`-`in` and of the caches of names, 2.01x
after the round of entering and leaving a function, 1.96x after the round of callbacks, 1.91x after the
round of what an appended text keeps of its end, 1.89x after the round of the text of a Number, and
stepping a generator without a result object took it to 1.88x. The arguments object in a stack slot halved
one case of the 108 and left the mean where it was, the string hash that follows from the hashes of
what is concatenated took it to 1.78x, more built-in functions run in place of a call to 1.71x, and
chains of `+` compiled as one concatenation to 1.68x, the arithmetic of doubles done as that to 1.67x,
objects copied by their shape to 1.65x, bound functions and rest parameters run in the loop of
instructions to 1.64x, holes looked for by blocks, Strings compared in the loop and `instanceof`
answered there to 1.63x, a generator resumed in the loop of instructions to 1.60x, the names of the
global object read by the state of the global scope to 1.59x, a property read and written by a
computed String in the loop of instructions to 1.57x, the variables of enclosing functions kept for a
call once it has read them to 1.534x, and derived class constructors and `super(…)` run in the loop of
instructions to 1.527x. One handler around a `for`-`of` loop, where every turn of it pushed and popped
one, took a fifth off the walk of an Array and left the mean where it was (1.529x, in a run where
QuickJS's own times were 0.5% lower than in the run before and Nova's 0.3% lower). Compound assignments
to properties compiled without their Reference pushed and popped took a method that counts on `this`
from 2.4x to 2.2x and the particle step under 3x, with the mean at 1.526x (these runs shared the
machine with other builds, and each engine's times read 1% higher than in the run before).

Joining the Android line left it where it was, at 1.91x then, by two things that cancel. That line counts what the
engine allocates for the memory limit, in an allocator of its own under every allocation (a header in front
of each block, a counter and a thread-local read): the cases that allocate most pay for it (a linked list
built and dropped 17% slower, array literals 15%, serializing to JSON 9%, numbers joined 9%; 2% over all
cases). And texts are compared by their pieces where the string table looks a new one up (area 7), which
halved the time of the rows of HTML (5.2x to 2.7x) and took a seventh off escaping HTML.

One case ran Nova out of memory before (`prop/destructuring`). With the count, 50,000 closures no longer
fit the 8 MB that the estimate let them have; they do not fit QuickJS's either, nor do 30,000 retained
objects, which Nova does hold.

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

Tried and dropped at the time: giving every plain instruction the length of its own to advance by, so that
where the next instruction is would not wait for the table of lengths. It read 13-32% slower on loops then.
Done again later (below), it is a gain; how the first attempt was written is not recorded.

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
fewer instructions a turn was the surer gain.

The round after took the next sequences of that count: an operator and the store of its value in a local or
the push of it (`ApplyBinaryOperatorAndMove`), the push of what an operator with a constant made (a bit of that
instruction), and the function of a method call with its Reference (`LoadPropertyWithCachePushReference`).
The run of every case is 215 million instructions, a quarter fewer than the 286 million, and `Load` is one in
twenty-one. The time moved far less than the count this time: 2.47x to 2.46x over all, the math cases 2.8x to
2.7x. An instruction that only moved a value cost little beside the dispatch, and the dispatch of the fused
one remains; what is left to fuse (the last argument and its call, at 2.5% of what runs) will give less still.

Then the step itself. Sampled on a loop of property reads it was three tenths of the time, 29 machine
instructions, and what the samples showed is a processor kept busy, not one waiting: the time of the
interpreter goes by how many machine instructions it runs, whatever they are. Three changes to the step:

- every arm moves the instruction pointer past its instruction itself, by a length that is a constant of the
  arm (the instructions of one arm are of one length, which is checked when the engine is compiled), where it
  was found by a table from the byte just read. One way of writing this kept the place of the instruction
  alive beside the pointer for the instructions that are not plain, and the pointer went to memory; as written
  now the arm of everything that is not plain steps by the table itself, and nothing but the pointer is
  carried from one instruction to the next;
- an instruction is read as a window of five bytes with one look at how much code is left, where its byte and
  its arguments had one each; the last instructions of the code, its end, and every instruction when they are
  printed go the old way, so the flag for printing is not read per instruction;
- the comparisons that jump, the tests that end a loop and the operators have an arm for each kind, in which
  the kind is a constant, where one arm served all of a family and asked the kind again what to do.

23 machine instructions are left between two instructions. All cases went from 2.37x to 2.21x, 62 of the 108
by 3% or more: loops 1.76x to 1.47x, calls 3.18x to 3.00x, properties 2.70x to 2.54x, the application cases
2.29x to 2.16x.

And what an instruction reads. A small integer or double is seven bytes of a value, and was made a u64 by
writing a zero and those bytes into an array of eight. The compiler made a vector shuffle of that: a dozen
instructions and a trip through a vector register for every operand of the arithmetic, every counter and every
index. Put together from the three pieces that seven bytes are loaded as, with shifts, it is a few
instructions on registers. An empty loop went from 2.3x to 1.0x, the loops together from 1.97x to 1.76x
(before the changes to the step, which were measured on top of this).

And what an instruction names. A constant, a name, a property cache or a binding site was read through the
heap's table of executables: to the Executable by its index, with a look at the table's length, and from
there to the constant, with another; a dozen machine instructions, and a turn of
`o.x = o.x + o.y + (o.z & 1)` on a closure variable makes eighteen such reads in its thirteen instructions.
The constants, the caches and the binding sites of an Executable are one allocation with its bytecode now,
held by count: by the Executable in the heap and by every `Vm` while it runs the code, whose loop keeps them
as three slices. The entries are cells, which the collector rewrites where they are, so the holders read what
is current. A call run in place takes one reference for all of it, where it kept the callee's bytecode in
the frame and compared it with the function's at every call (area 3). A loop over a closure variable went
from 2.2x to 2.0x, a property read by a computed name from 2.8x to 2.5x, the loops from 1.46x to 1.40x,
calls from 3.00x to 2.95x, all cases from 2.21x to 2.18x. Kept as a second reference beside the bytecode's,
the tables had made calls up to 10% slower.

Left: an integer is still read as three loads and written as four stores (its seven bytes do not make one
machine word without the byte of the value's kind); the step is 23 instructions of which seven are the jump
through the table; a property the cache places is read through two calls and a result that is taken apart
again (area 4). The counts of pairs say what could still be fused, and the round before showed how little
that gives now.

Counted again over the 36 cases at or over 1.9x (what each executes, by adjacent pairs), the commonest
pair that could be one is a value fetched and a property of it read by the cache: a variable of an
enclosing function or a global, then `GetPropertyWithCache` or the function of a method call (`state.x`,
`Math.floor(…)`; a quarter of the instructions of `prop/get-set-one-shape`, a sixth of those of the
cases that call `Math`). Tried: the arm that fetches looks at the next instruction and, where it is such
a read, a function of its own does it at once. That gained nothing where it was for (2% slower) and cost
the loops and the calls 2-5%: going from one instruction to the next is about 0.7 ns here (the lines of
the dispatch in the profile, over the instructions of a turn), which the call, the second look at the
instruction and the tests cost again. And given the loop's tables (three slices, passed by address) or
its instruction pointer to move, that function made every case 17-34% slower, a `switch` 34%, an addition
of integers 23%: the loop keeps in memory what it lends. Dropped.

What a turn of those cases costs is the reads themselves. A property the cache places is about 70
machine instructions (the object, the cache, the shape again for the capacity and the length of its
storage, a jump by the capacity among twelve vectors, the slot), 2.9 ns, and an assignment 3.4 ns, of
which 0.9 is the descriptors of the storage looked up in a hash table, whenever any storage of that
capacity has descriptors. Together they are 43% of `prop/get-set-one-shape` and a third of the particle
update, where the numbers that do not fit a value and the collector are under 6%. The descriptors are
handed out as entries of that table in some 170 places of twelve files; knowing without the lookup that
a storage has none is a change to all of them.

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

The walk from the running Environment Record to the one a site remembers was a function of its own, called
for every such read, and its result (which kind of record, which one) was taken apart again by its caller; it
is part of the few functions that read or write at a site now. A loop over a closure variable went from 2.3x
to 2.2x, one over a global from 3.0x to 2.9x.

A read of a property of the global object was still about 8 ns against 3 for a closure variable: the way
from the running realm to the slot was a dozen loads, one through the other (the realm, the global record,
its two halves, the object, its shape, the keys, the values), and a write asked two hash tables on top of
that whether the property is writable. What the place of a name there depends on is remembered now as one
state of the global scope (`GlobalScopeSeen`): the realm, its global object and that object's shape; how
often the keys of an intrinsic object's shape were moved in place, which is what the default global object
has (a property removed from among the others, private names put in front of them all; a property added
goes to the end and moves none, so new globals cost nothing); the global lexical record and how many
declarations it has; and the collection the handles are of. The states seen are numbered, and a site among
the properties of the global object keeps the number of the state its place was found in, where it kept
the count of declarations. The place is good while that state is the one seen and the global scope is in
it still, which six comparisons of values at hand tell, none read through another. When any of it changes
the next use of a site finds the name the long way under a new number; a name that is only assigned to
finds its place again then as well, which it had no need to before. `loop/global-variable` went from 2.8x
to 2.2x, a call of `Math.max` from 2.3x to 2.1x, `math/floor-round-minmax` from 2.1x to 1.9x,
`math/random-and-date` from 2.2x to 1.9x; `math/` from 2.0x to 1.9x.

The check had been the name at the slot and not the shape because that shape changes in place; the first
plan of this round compared the shape alone, and the notes above were what said why that is not enough.

As first built the check for a global, short now, was compiled into every read of a closure variable (the
two are one function, and its global part had become small enough to be made part of it): a call of a
closure was 4.5% slower and a text appended in a loop 7%, in the two binaries run in turn, with every gate
passed. The global part is a function of its own, for a tenth of the gain on a global. Three cases that
read closure variables still read 2-4% slower in the binary of this round than in the one before
(`app/particles-step`, `array/two-dimensional-grid`, `prop/get-set-one-shape`), with nothing they run
changed; a build with the new state behind a pointer moved the same cases by -2% to +1%, which is what
where the code and the Agent's fields lie does. Not followed up.

A read of a variable of an enclosing function stayed five loads, one through the other (the running
Environment Record, the function's, its declarative one, the bindings, the binding), about 1.6 ns, and a
tenth to a fifth of every case whose loop names an object, an Array or a function of its surroundings,
which is how the cases are written and how widget code is. Where such a variable has one value for as
long as a call lasts, the call reads it from its Environment Record once and keeps it in a stack slot of
its own: the compiler looks through the loops of a function for the names of variables of enclosing
functions (up to eight), gives each that qualifies a slot that starts as undefined, and compiles every
read of it in the function to an instruction that takes the slot's value when it is not undefined and
otherwise reads the variable where it is and keeps what it read (`GetBindingAtSiteOnce`). Undefined is
nothing kept: a `var` before its initializer, and a variable that is undefined, are read each time.

It qualifies where nothing can change it during a call of the reader, which is what
`bytecode_compiler/kept_captures.rs` says at length: the reader is a plain function (a generator's or an
async function's call goes on after other code has run); the variable is declared outside it (one
declared in a block of the function itself is another variable in every turn of a loop); nothing assigns
to it (no reference writes it, and no direct `eval` is anywhere in the declaring scope); and its
declaration gives it its value once (a `let`, a `const`, a class) or only while no call of the reader
can be going on (a `var`, a function declaration, a parameter of a plain function: a generator or an
async function that declares it can be resumed from inside the reader, and its `var`s keep nothing).
The script written for it goes at each of these, and at the stack around the slots.
`prop/get-set-one-shape` went from 3.2x to 2.5x, `array/index-read-write` from 2.5x to 2.0x,
`typed/pixels-rgba` from 2.6x to 2.25x, `typed/float64-math` from 3.3x to 2.9x,
`prop/dynamic-string-key` from 1.5x to 1.2x, a grid of Arrays from 1.7x to 1.5x; properties together
from 1.6x to 1.5x, typed arrays from 2.5x to 2.3x, all cases from 1.57x to 1.53x.

Left: `typeof name` still goes through a Reference. The code of a Module and of an `eval` is compiled as
before, and module variables are resolved the long way. A variable of an enclosing function that is
read outside a loop, or assigned to anywhere, is read where it is each time (a recursive function's
calls of itself, `call/recursive-fib`; a counter that closures share). A `let` or `const` of the top level is found
through the realm at every read (2.0x in a loop). A write of a global asks the descriptors of the global
object's storage whether the property is writable, two hash lookups: its attributes change with nothing a
site could see.

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

A call of any other built-in function still cost several times what the function did: by profile, a loop
that takes two substrings of a String was a tenth in the substrings and most of the rest in getting to them
(the arguments copied to a list, the `Vm` put aside for the collector, the execution context, the depth of
calls counted, the arguments converted). Many functions, for the this value and arguments they are nearly
always called with, run no script, throw nothing and need no collection, and for those the interpreter now
does what the call would have done, in place: `slice`, `substring`, `charAt`, `startsWith`, `endsWith`,
`indexOf`, `includes` of a String with integers and Strings; `repeat`, `padStart` and `padEnd` where the
text is short; `push` of one value to an Array to which an element is simply appended, and `pop` of its
last element; `Array.isArray` of anything but a Proxy; `get`, `has` and `set` of a Map, `has` and `add` of
a Set; `hasOwnProperty` of an object that answers without running anything, for a key that is one as it
is; `String(x)` and `Number(x)` of a primitive; `toFixed` of a Number; `Math.random` and `Date.now`; and
`f.call(thisArg, ...)` where `f` is one of these. Each shares its code with the function, which runs it
after converting what it was given; anything else (another kind of this value or argument, a function put
in its place, the function reached some other way than by `o.f(...)`) takes the call.

`push` and `pop` in a loop went from 2.9x to 1.8x, `startsWith` and `endsWith` from 2.8x to 1.8x,
substrings from 2.9x to 1.9x (outside ASCII 3.2x to 2.4x), `Math.random` and `Date.now` from 3.1x to 2.2x,
a Set from 2.1x to 1.5x, a Map from 2.1-2.4x to 1.6-2.0x, padding and `repeat` from 2.8x to 2.0x, an array
of strings pushed and joined from 3.3x to 2.4x, Numbers to text from 2.4x to 1.9x, `instanceof` with
`Array.isArray` from 3.7x to 3.0x, `hasOwnProperty.call` with `in` from 2.8x to 2.3x. All cases went from
1.78x to 1.71x.

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

Sampled again after the instructions around a call were fused (a call with a computed argument is five
instructions in the caller where it was seven): entering and leaving the function are 41% of a plain call,
two functions of 682 and 220 machine instructions with the samples spread over them, and no one step stands
out. The
return no longer goes the general way, and a frame kept the callee's bytecode for the next call from its
depth instead of counting a reference up and down each time; neither moved the time by more than 1%. (The
bytecode is held with the tables of area 1 since, by one count that is not shared between threads, and the
frame keeps nothing for the next call.)

The exchange of the stacks was looked at by machine instruction after the rounds on area 1. Entering wrote
the arguments to the function's stack and then exchanged the four stacks, and leaving emptied the four and
then exchanged them; the exchange moves two words at a time, and a word written just before is then read
with its neighbour, which waits for the write. Exchanged first and written to after, a plain call went from
11.5 to 10.7 ms and the recursive Fibonacci from 5.4 to 4.9 ms. Entering and leaving are 230 and 110 machine
instructions on the usual way, 43% of a plain call, with the samples spread evenly: no step stands out, and
making both part of the loop of instructions instead of functions of their own was 5-12% slower on calls.

Left: per call in place, the execution context (pushed and removed), the frame with the four stacks swapped
into it and back, a rooted handle of the callee's executable; under them the dispatch of area 1. A stack
shared by caller and callee, with the arguments left where they are, would take most of the frame away; it
changes how every instruction finds a stack slot, and the three other stacks would have to be shared with
it or still be swapped (an instruction that binds parameters looks at the top of the iterator stack, which
would be the caller's). Generators are 6.1x. Next: `new` of a plain constructor, and the getter or setter of
a property, in place. Reach: about 2.5x.

Every ordinary function that has been compiled runs in place now, not only one that keeps its parameters in
its first stack slots and needs no Function Environment Record. A function that binds its parameters with its
own instructions (a default value, a rest parameter, a pattern, an arguments object) finds the list of its
arguments on the iterator stack as it does in a `Vm` of its own: the list is a scoped collection of the
Agent, made when the function is entered and let go of when it is left. A function that needs a Function
Environment Record (an arrow function in it uses its `this`) gets the record as \[\[Call]] makes it. A
function with a default and a rest parameter went from 4.4x to 3.7x, one that reads `arguments` from 5.2x to
4.7x.

And a function with a parameter expression made up to three Environment Records at every call that only a
direct eval, a function made in the parameter list or a var of the body could tell from the one its
parameters are in (steps 20, 28 and 30 of FunctionDeclarationInstantiation). With none of the three in the
function, none of the records is made, and the function is called as one without parameter expressions is;
the two that only a direct eval observes are made only where there is one. The same function went on to
2.7x. Calls together are 2.73x (2.86x), all cases 2.14x (2.16x).

`new F(…)` took the native way for every function. Where F is not a derived class constructor and its
code needs no Function Environment Record (no new.target, no super, no arrow function that uses its this),
the constructor runs in place as a call does: the object is made from the function's "prototype", is the
this value of the call, and waits on the caller's stack for the constructor to return, since what `new`
gives is that object unless the constructor returns another. A class constructor had been compiled as
always needing the record; its body says what it needs now. `new` of a class with two fields set went from
3.1x to 2.3x. Derived classes, and `new` with spread arguments, take the native way.

Left there: binding through the iterator instructions is the general way for each parameter, a rest
parameter allocates its Array, and the arguments object is made in full at every call and kept in an
Environment Record (4.7x; it has a stack slot now, below).

Where a call's time was after that, in a closure called 300,000 times (`call/closure`, 3.3x): 53% in the
loop of instructions, 20% entering the function and 12% leaving it, 15% reading and writing the two
variables it closes over. Entering and leaving had no one place in a profile by function or by address.
A build of the benchmark with line tables (the release profile with `debug = "limited"`, a dSYM, and
`atos -i` for each sampled address) gave them by source line, and three things stood out that were work
for nothing:

- The loop read the executable of its code through the rooted handle whenever it was entered, which is
  after every call and every return: 5% of the case. The code that starts or goes on comes with its
  executable now, and the handle is asked after a collection.
- As a function was left, the Agent's scoped values were cut back to under the caller's executable and
  that was rooted again where it had just been: the frame keeps the caller's handle instead, and the
  scoped value stays.
- The function had a Reference stack, an iterator stack and an exception handler stack of its own,
  exchanged with the caller's at each end and emptied after, though most functions push to none of them.
  It shares the caller's now, above where the caller left them (an error goes to the handlers above the
  function's base, then leaves it), and only the value stack is exchanged: a function's instructions
  name the slots of its variables from the bottom of that one, so a base for it would cost an addition
  at every local.

A plain call went from 2.8x to 2.4x, a method call from 2.8x to 2.6x, a closure from 3.3x to 2.9x,
recursion from 2.7x to 2.3x, a getter and a setter from 3.0x to 2.6x, and the group from 2.6x to 2.5x.

What is left of entering and leaving, a quarter of a plain call that does nothing else: the execution
context pushed and removed (a third of entering), and after it nothing larger than a twentieth: the frame
looked up and written, the tables of the code taken and let go of (a count up and a count down), the
function's executable rooted, the this value taken from the call's Reference, the value stack exchanged
and emptied.

A function that reads `arguments` (`call/arguments-object`, 4.8x) paid for the object three times at
every call. Its shape was looked up in the realm's table of shapes, key by key. It was put in a Function
Environment Record that the function had for nothing else, and found there by name. And `arguments[i]` and
`arguments.length` went the general way of a property of an exotic object.

- The Agent keeps the shape of an arguments object of up to eight arguments, by length and prototype (a
  table the collector empties, so it holds nothing alive).
- The object is in a stack slot where nothing but the function's own code can name it: no arrow function
  in it says `arguments`, there is no direct eval in it, and it declares nothing of that name (a
  parameter, a var, a function). A reference is the slot's if what the parser resolved it to is not
  declared inside the function: a `let arguments` in a block is that block's, while an outer
  `var arguments`, which the parser resolves a function's own `arguments` to, is not what the function
  sees (test262 has it; the first build of this failed three of its tests on it).
- A read by index takes the element from its place among the object's properties (three come before the
  elements, then one per argument in order) if the key at that place is still that index and it holds a
  value; `length` goes through the instruction's cache as a property of an ordinary object does. An
  element deleted or redefined, or anything else, goes the general way.

The case went from 4.8x to 2.3x, and calls from 2.4x to 2.2x. What is left in it is the object itself,
made at every call with a property per argument.

A bound function (`f.bind(o)`) was called the native way: its \[\[Call]] calls the function it was bound
from, with a `Vm` of its own for that. Its \[\[Call]] is that function's call with another this value;
where it has no bound arguments and the function is one the `Vm` runs in place, the call instruction
runs the function in place and gives the call a Reference whose base is the bound this value, which is
where a call takes its this value from. A bound function with bound arguments, or of any other kind of
function, takes the call as before. `call/bound-function` went from 2.5x to 1.8x.

A function with a default value or a rest parameter binds its parameters by instructions that step
through the list of its arguments, and those went the general way of an instruction each: a step of the
list as of any iterator, a rest parameter's Array made empty and filled a property at a time. Where the
list is the call's arguments, the step, the Array of what is left (of up to 16 arguments) and the end of
the list are done in the loop of instructions. `call/rest-and-default` went from 2.7x to 1.7x; calls
from 2.2x to 2.1x.

Where calls are after that: a plain call and its return take about 14 ns of entering and leaving
against QuickJS's 14 ns for the whole step of the loop, and no line of the two is above a twentieth of
the case (the execution context pushed, the frame found and written, the stacks exchanged, the this
value, the executable rooted). (Every step of a generator took the native way too, until a loop resumed
it in place: area 5a.)

`new` of a derived class and `super(…)` still started a `Vm` each: `new B(i)` where B extends A was two
of them, and with a method call after it 320 ns against QuickJS's 113 (`call/class-inheritance-super`,
2.8x). Both are run in the `Vm` of the calling code now, where the constructors are functions of the
script. A derived class constructor is entered with its Function Environment Record, which has the new
target and no this value; when it returns, what it is constructed to is what \[\[Construct]] makes of
it (the object it returned, else the this value its record has by then, else the errors of steps 10 and
12, thrown by the calling code once the function is left). `super(…)` finds the parent as SuperCall does
(the prototype of the function whose record gives `this`, and that record's new target), runs a parent
that is no derived constructor as `new` of it is run, on an object made from the new target's prototype,
and a derived one as above; when the parent returns, the rest of SuperCall is done by the function that
\[\[Construct]]'s way uses too (the this value bound, the instance elements initialized). A built-in
parent, a bound function, a parent that needs a Function Environment Record without being a derived
constructor, spread arguments, and a parent that is no constructor go the general way. The case went
from 2.8x to 2.0x.

That is in the function every instruction runs through, and what the compiler makes of that function
moved with every way of writing it: `super` as one more arm of the plain instructions made an empty loop
6% slower; looked for before every instruction that is not plain, the same; with the new target carried
in what a call hands to its entry, plain loops were as before and plain calls 4-7% slower. As it is,
`super` is a result of the general way that the loop looks at where it looks at `yield`, the frame
remembers the constructor's record, and the result is settled where a return examines the frame anyway:
the loops and the calls are where they were in the whole benchmark (each within 1.5%), and `new` of a
class that is not derived reads 3% slower in the two binaries run in turn (`call/new-class-instance`,
1.94x to 2.0x), with nothing in its path changed but one comparison. The same binary run against itself
moves a plain call by 4% from run to run.

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

A read where the cache knows the shape was three calls: the cache searched by one function, the property read
by another through a third, which returned the slice of the storage for the caller to index; and what came
back was made a result of several kinds, written to memory and taken apart again. About sixty machine
instructions. It is one function now for the object's record, the search of the cache and the one slot, and
the assignment is another. `o.x = o.x + o.y + (o.z & 1)` went from 3.3x to 3.1x, a method call from 3.1x to
2.8x.

Left: a property read is the cache (up to four shapes compared), then the object's record, its shape's record
for the capacity of the storage, a jump on that capacity to the arena of that size, and the slot: about eight
dependent loads where QuickJS has three. The loop of `o.x = o.x + o.y + (o.z & 1)` is 3.2x.

An accessor was a call made the native way. Where the cache of the place knows the shape and the property
is an accessor, of the object or inherited, whose getter is an ECMAScript function, the read is a call of
the getter that the `Vm` runs itself (no arguments, the object as the this value, what it returns as the
result); and the assignment is such a call of the setter, whose result the frame drops. A class with a
getter and a setter went from 4.7x to 2.9x. Reads that keep a Reference for a call that follows
(`o.getter()`) take the general way still.

`v instanceof F` looked F[Symbol.hasInstance] up key by key, called the method every function inherits as
a built-in, and that looked F.prototype up key by key. Both lookups go through the property cache of
their name, and the inherited method of the running realm is done without the call: 4.4x to 4.0x. What is
left of that case is the call of `Array.isArray`. Functions, Arrays and other objects with
a backing object do not take the short way for their named properties yet.

Those lookups, "prototype" at every `new` run in place, and the ones a walk over an Array makes (area
5a) each asked for the property cache of their name, and that was found in a table hashed by the name's
text under SipHash: a tenth of `prop/destructuring`. The caches given last are remembered now with their
keys (64, two places for a key by its identity), and found again by comparing the key; a collection, which
moves keys and caches, empties that memory. `instanceof` went from 4.0x to 3.6x, `new` of a class from
2.3x to 2.1x.

The instruction itself still left the loop of instructions for every `instanceof`, to root its operands
for a method that might be a script. It is answered in the loop now where nothing can run: the target is
a function, not a bound one, whose @@hasInstance is the inherited method of the running realm; its
"prototype" is a data property that holds an object; and the chain up from the value has no Proxy in it.
Anything else, and everything that throws, goes the way it went. 2.9x to 2.6x; the rest of that case is
the calls around it.

`for`-`in` put every key it met in a set and looked every key up there first, hashing each name twice
under SipHash, to learn nothing in nearly every loop: the keys of one object are all different, and none
was met before the first object's. The set is asked now only for an enumerable property of an object
that is not the first, which most prototypes have none of, and the keys wait in a list until it is; and
the steps are taken in the interpreter where no object on the way is a Proxy (the step that runs nothing
was there, unused, and lost a key where it had to give up). `prop/for-in-and-keys` went from 2.1x to
1.1x. What is left of it is one lookup by key for each property, to see that it is still there and
enumerable, and the read of the value by a computed key in the body.

An object pattern with no default value and no computed key (`var { x, y, meta: { id } } = point`) was run
by instructions of its own, an interpreter inside the instruction: every property looked up by its key,
every name resolved by its string, and the keys gathered in a set whether or not a rest element wanted
them. The compiler had the other way already, for a pattern with a default: `o.p` read by the instruction
that reads it anywhere, through the property cache of its place, into a stack slot where the name is kept
in one. Every object pattern is compiled that way now. `prop/destructuring`, an object pattern and an
array pattern, went from 3.7x to 2.0x with that, and to 1.5x with the array pattern's part (area 5a).

`{ ...base, e: i }` and `Object.assign({}, last, { f: 1 })` (`prop/object-spread-assign`, 3.2x) each
went the whole way of the specification for every property: the list of the source's keys, a descriptor
made of each property, the value read, and the property defined or assigned on the target. There was a
shorter way for the spread, a copy of the source's shape and values, that never ran (below). A source
whose properties are all plain data properties under String keys, which is nearly every object that is
spread, now gives an empty target its shape and a copy of its values: the copy is what the steps make
(the same keys in the same order, each property writable, enumerable and configurable, the target
extensible whatever the source is). `Object.assign` assigns the properties of such a source one by one
with no list of keys and no descriptors, where the target takes each without a setter or a trap; where
it does not, the steps do all of it from the first key, and what was assigned is assigned again, which
nothing can see. The case went to 1.9x.

`key in object` looked for the key among the object's own keys one by one, where every other lookup of
an object with 32 keys or more uses the table of its shape's keys. An object used as a dictionary of 500
keys, with `in` at every step (`prop/dictionary-churn`, 2.8x), spent a third of its time there; it went
to 1.8x. Properties together went from 1.9x to 1.8x, all cases from 1.67x to 1.65x.

Tried on the way and not kept: the keys of a small object compared as the 64 bits each is, in place of
kind by kind and field by field. It read a twelfth slower (`prop/dynamic-string-key`, where a computed
String key is looked up among eight; a fifth of that case is the search, and it is 2.6x).

That case was not the search, though. `o[name]` with a String for the key, and `o[name] = v`, had no
short way at all: the instruction that reads an element took only an integer, and everything else made a
Reference of the object and the key, went through GetValue to \[\[Get]], and there had a property
descriptor made of the property found (its attributes asked of the descriptors' table) to take the value
out of; three fifths of the case. For an ordinary object that has the name as a data property of its own
the read is that property's value, and the assignment sets it where it is writable (what OrdinarySet
comes to with the object for the receiver): the key is found among the object's keys (a search of a few,
the table of a large shape) and the value read or written at its place, with no descriptor. A name the
object does not have itself, an accessor, a property that is not writable, and every other kind of
object go as before, and give what they gave. A String that is the text of an integer is that integer
for a key, as everywhere. `prop/dynamic-string-key` went from 2.6x to 1.5x, `prop/dictionary-churn` from
1.9x to 1.4x, an object used as a dictionary from 1.9x to 1.5x, `for`-`in` with `o[k]` in its body from
1.05x to 0.86x, an event emitter that keeps its listeners by name from 2.1x to 1.8x; properties together
from 1.8x to 1.6x.

The instructions are the ones that read and write an element by its index, and as first written the
String was one more arm of their choice of key: a grid of Arrays read by two indexes was 5% slower and
an Array read and written by index 2.5%. The integer's way is as it was, and the String is taken where
that way finds no integer, in functions of their own.

Left: a name the object inherits, and a name it does not have (`undefined`), still go the general way; a
small cache by shape and key would answer them, and would save the search.

`target op= value` reads the target with its Reference kept and assigns through that Reference. While
the value was evaluated the Reference was pushed, and popped after, unless the value was a literal:
`this.n += by` was seven instructions. Two ways around the push and the pop, both in what the compiler
emits and neither in the loop of instructions.

A value whose code leaves the reference register alone is evaluated with the Reference in the register,
as a literal was: a variable that the function keeps in a stack slot (one instruction that reads the
slot, or throws in its temporal dead zone), and the arithmetic, the comparisons and the choices among
such and literals. Their operators may run code of the script (a `valueOf`), which another `Vm` runs;
the operator of the compound assignment itself always ran with the Reference in the register. A
property, a call, a name found elsewhere, an assignment, `in` and `instanceof` in the value keep the
push and the pop. This is for every target that has a Reference (a property by name or by key, a name
found by `eval`), and for logical and plain assignment to such. `this.n += by` is five instructions.

`p.x op= value` with any other value, for a `p` in a stack slot and where nothing uses the value of the
assignment, keeps the object on the value stack as `p.x = value` does: the object is pushed, its
property read as any other, and one instruction assigns by the cache. The object assigned to is the one
whose property was read (the slot is read twice with nothing in between, before the value can replace
the local), and what is read and assigned are \[\[Get]] and \[\[Set]] of that object in the order they
had. `p.x += p.vx` is six instructions for seven, and makes no Reference.

With the two binaries run in turn, a method that adds its argument to a property of `this`
(`call/method`) went from 14.7 to 13.9 ms (2.39x to 2.21x) and the particle update, which has two
`p.x += p.vx` in a step, from 4.47 to 4.34 ms (2.8x). Nothing else moved outside its noise.

Left: `this.x op= this.y` is as many instructions either way and was left; a compound assignment whose
value is used, and one to an element, still make the Reference.

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

Looking at the two prototypes' keys on every call was 3-5% of the application cases that push: about fifty
keys each time.
Both have intrinsic shapes, and a key is added to such a shape in one place (`ObjectShape::push_key`), which
now notes on the heap that an integer key was added to one. Until one was, an appended element looks at no
keys; after, at all of them as before (the note is never taken back, and says nothing of which object).
`array/push-then-pop` went from 3.6x to 3.0x.

An array literal of values (`[i, i + 1, i + 2]`) made an empty Array and defined its elements one by one,
each through CreateDataPropertyOrThrow by an instruction that may run a script (the `Vm` moved into the
Agent and back for it), inside an exception handler that pops the Array where an element throws. The
values go to the stack now, as the arguments of a call do, and one instruction makes the Array of them.
A literal with a hole or a spread element is made as before. `array/literal-alloc` went from 3.7x to 0.9x.

A method that calls a callback for every element (`forEach`, `map`, `filter`, `some`, `every`, `reduce`,
the find methods, the comparator of `sort`, the mapper of Array.from) made a call from native code each
time, and that is the costliest way to call: a `Vm` made for the call and let go of after, the kind of
the function and the way its code takes arguments found out again, its executable rooted, and around it
HasProperty and Get of the element by its key. Two fifths of a `forEach` over a thousand elements was
that, and less than a third the callback's own instructions.

Such a method keeps one `Vm` for all its calls now (`RepeatedCall`): what kind of function the callback
is, is found out once, and a call is the execution context of the call, the arguments put in the
function's parameter slots, and the code run. It is for the functions the interpreter could run in place
of a call instruction (ordinary, compiled, parameters on the stack, no Function Environment Record); any
other callback is called as before. And an element that is there as a plain value is read where it is,
which is what HasProperty and Get of it come to. `forEach` with an arrow function went from 3.1x to 2.0x,
`map`, `filter` and `reduce` in a chain from 2.8x to 1.8x, `find`, `some` and `every` from 2.9x to 1.8x,
a sort with a comparator from 2.6x to 1.9x.

Get and CreateDataPropertyOrThrow, as built-ins call them, went straight to the internal method that can
run a script, which roots its operands first. Both look without a script first now, which answers for
every data property and every missing one, and for every object that is not a Proxy; the internal method
is for a getter and a Proxy. `fill` wrote directly only into an Array with no holes, so
`new Array(n).fill(v)`, which is all holes, assigned element by element; a hole is written like an
element where assigning to it comes to making it (`Array::appends_plainly`). With the mapper of
Array.from called like the other callbacks, `array/from-and-fill` went from 3.9x to 1.6x.

An element made by a path that writes storage directly has to be counted for the heap's size where it is
made: the storage of an Array is not counted when it is allocated, only as elements are defined. `fill`
over holes counts them, and so does the instruction that makes an array literal, which had not since it
was added (an Array of a hundred thousand filled holes was 800 KB the memory limit did not see until the
next collection; a test of the limit caught it). `array/literal-alloc` is 4% slower for the collections
that are due sooner.

`shift`, `unshift` and `splice` ask first whether the Array has a hole, of all of it and every time, and
that was a loop that stops at the first hole: one element, one branch. Asked of blocks of 64 elements
with no way out inside a block, it is compiled to compare several elements at once, and an Array with a
hole is still known within the block that has it. `array/shift-unshift-splice` went from 2.3x to 1.4x.

Left: a compound assignment to an element (`a[i] += 1`) and one whose value is used still make a Reference.
The same methods of typed arrays, `flatMap`, and `forEach` of Map and Set call their callbacks the general
way; so does every single call from native code (a bound function, `call` and `apply`, a getter reached
from a built-in). The result of `map` and `filter` is still defined element by element. Float64 elements
allocate (area 6).

### 5a. Iteration

`for (var ch of string)` was 10x: every step called %StringIteratorPrototype%.next, which made an iterator result
object (through two vectors allocated and freed for its two entries, a third of the time), and the loop then read
`done` and `value` back with two general property lookups. A loop over a generator, a Map or any other iterator
did the same reads.

Done: an object made from a list of entries gathers their values on the stack; a String Iterator whose `next` is
the original one is stepped directly, as an Array Iterator already was; and the `done` and `value` of a result
that is an ordinary object with those as plain properties of its own are read from their slots. Iterating a
string by code point is 3.7x, a Map 2.7x, a generator 7.0x (8.2x).

Iterating an Array is a walk over its elements with no iterator object, and three things around the walk
cost more than it: the Array's `Symbol.iterator` method was looked for key by key, on the Array and on its
prototype, as every walk began; "return" was looked for through three prototypes as every walk ended early,
which a pattern with no more elements than its array always does; and every step left the interpreter,
moving the `Vm` into the Agent and back. The method is found through the property cache of its name now,
and so is there being no "return"; a step whose element is a value reads it in place. `for`-`of` over an
Array went from 1.8x to 1.3x, a literal with spread elements from 2.2x to 1.8x, a call with spread
arguments from 1.8x to 1.5x, a Map iterated with a pattern for each entry from 2.4x to 1.9x, and the array
pattern of `prop/destructuring` took that case from 2.0x to 1.5x.

The walk was taken wherever the Array's method was the original one, whatever "next" of
%ArrayIteratorPrototype% had become; it is taken now only while that is the original too (see the
defects), which is looked at as each walk begins: two keys compared.

The two lookups by name hashed the name every time, the start of a walk left the interpreter to make
them, and the step of `for`-`of` was reached through the instructions that may run a script. The caches
of names are found by the key now (area 4), the start asks where it is, and the step is among the
instructions the loop runs itself: `for`-`of` over an Array is 1.0x (1.3x), `prop/destructuring` 1.3x
(1.5x), a call with spread arguments 1.3x (1.5x), a Map iterated with a pattern 1.7x (1.9x).

Left for Arrays: an array pattern is still run by instructions of its own, an interpreter inside the
instruction that reads them through the heap (a tenth of `prop/destructuring`).

A generator made its result object in its own bytecode at every `yield`, the loop called `next` through the
built-in to get it, and read "done" and "value" back out of it. `yield value` in a generator that is not
async is an instruction of its own now (YieldValue) that hands the value over as it is, and whoever resumed
the generator makes the result object if it wants one: %GeneratorPrototype%.next, `return` and `throw` do,
with the %Object.prototype% of the generator's realm as its bytecode did; the step of a loop, and the step
that `Array.from`, the Map and Set constructors and the Promise combinators take, resume the generator
themselves where its `next` is the original one of the running realm, and take the value. What `yield*`
passes on is a result object already and is read as any other. A loop over a generator went from 9.9 to
6.8 ms (5.9x to 4.1x).

(A suspended `Vm` keeps its stacks as vectors now instead of cutting them to size at every `yield`; that
removed two reallocations a step and changed no measured time.)

A step of that loop still started a `Vm` for the generator: its saved stacks moved into a new `Vm`, the
loop of instructions entered anew with all that its start does (the checks of the stack and of the
interrupt, the code and its tables looked up), and at the `yield` the `Vm` taken apart again; and the
generator's state (four stacks, the execution context, about 200 bytes) was moved out of the generator's
record and back at every step. Seven tenths of the case were that, three tenths the code of the loop and
of the generator. The step of a `for`-`of` and of a spread (`[...g]`, `f(...g)`) resumes the generator in
the `Vm` that runs the loop now, as a call runs a function in place (area 3): the calling code is put
aside in a frame, the generator's own value stack is the `Vm`'s while it runs, what it kept on the other
three stacks goes above what the caller has there, its execution context is pushed, and its code goes on
where it yielded. A `yield` does the reverse and its value is the value of the step; a `return`, or an
error the generator does not handle, completes it and leaves the frame as a function's is left, the
error going to the caller's handlers. The suspended state is a record of its own that the generator and
the frame hand to each other, so that nothing in it is copied; `next`, `return` and `throw` called the
general way keep the same record through their run and move less than they did.

It is taken only where the step would call the original `next` of the running realm on a generator that
is suspended, and one more call is not one too many. A generator that is running (it steps itself: a
TypeError) or completed, a `next` of one's own, and every other way of resuming (`next()` by hand,
`yield*`, a pattern, `Array.from`) go as before, on the same state: a generator can be stepped by a loop
and by hand in turn. The interrupt is asked and a due collection run at every resume, as at a call.
`call/generator` went from 6.5 to 3.6 ms (4.2x to 2.2x).

Left: a turn of that loop is 34 ns against QuickJS's 16. About half is the ten instructions that the
loop and the generator run for it (twelve before the loop had one handler, below; one is the jump that
follows every `yield`), the rest entering and yielding: the frame, the
execution context moved to the stack of contexts and back, the code's tables held and let go, its
executable rooted. Tried and dropped, each measured with the two binaries run in turn: an arm of the
fast loop for `yield` (1-2% of this case, and `loop/switch` 14% slower: see the defects); the three value
stacks exchanged with each place read before it is written; the one handler of a `yield` inside a loop
moved by itself; the generator's executable kept current by the collector in the frame instead of read
again at each `yield` (none of the three measurable); the jump after a `yield` taken at the resume (5%
slower).

A `for`-`of` loop pushed a handler after every step and popped it before the next, and a `for`-`in` loop
did the same. What the binding or the statement throws closes the iterator (its `return` is called), and
what the step throws must not: the handler was around the one and not the other. A loop has one handler
now, pushed before its first step and popped where the loop is done, and a step that throws leaves the
loop's iterator record exhausted before its error goes to that handler: closing an exhausted record
asks for no `return` and calls nothing. That is done in the three places the step of a loop throws
from: the general step (`next` throws or gives no object, reading `done` or `value` throws), a
generator resumed in place that throws, and the read of the result that a `yield*` in it passed on.
What leaves the statement by `break`, `continue` or `return` finds the same handlers above it as
before, and `continue` pops none. `for await` keeps its handler around the turn: its step is several
instructions, with an Await among them. Array patterns and spreads step without a handler, as they did.

Two instructions fewer in every turn: `for`-`of` over an Array went from 1.94 to 1.58 ms (0.97x to
0.77x), a string walked by code point from 1.15 to 1.09 ms (2.09x to 1.96x), a generator from 3.51 to
3.40 ms (2.23x to 2.15x), `for`-`in` with `Object.keys` from 2.37 to 2.29 ms.

What it cost: `loop/empty-for`, which runs none of this, is 3% slower in the binary that has it (3.29 to
3.40 ms with the two binaries run in turn; 0.98x to 1.01x), by where the loop of instructions landed.
Written with the marking inside the loop of instructions it was 7%, then 3.4% with one of the two
places moved out; both are functions of their own now, called where the step was called before, and
the loop of instructions has no new code. No other case moved outside its noise (the ones the full run
showed 3-5% slower were run again, binary against binary: within 1%, but for the step of the particle
animation, whose runs overlap and whose best is 4% apart).

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

The text of a Number was the other half of what a script that shows numbers does. Sampled, `String(i)`,
`i + ""` and `(i * 0.37).toFixed(2)` had a sixth of their time in std's exact formatting of a float, and the
rest of the conversion put its text together in the heap: an integer was formatted into a `String`, a double
into three (what `{:e}` writes, its digits, the result), each then copied into a JavaScript string or thrown
away for one of seven bytes. The text is written in the native stack now, and a String of up to seven bytes
is made of it with nothing allocated. `toFixed` takes its digits from one integer where one holds them, for
|x| under 2^53 and up to 20 fraction digits: x is m * 2^e with e <= 0 there, m * 10^f is under 2^120, and the
integer nearest to x * 10^f (the larger on a tie, as the specification asks) is that product shifted down and
rounded by what the shift drops; any other x goes the way it went. The same text goes into `JSON.stringify`
and `join`, and into the keys that are integers. Both are held to what they replace over some hundred
thousand Numbers of every kind (`number/text.rs`), and a script of three thousand gives what it gives in
Node. Numbers joined went from 1.9x to 1.3x, the three conversions from 3.3x to 2.4x, a frame of an easing
animation (which writes its values with `toFixed`) from 2.9x to 2.1x, template literals from 1.9x to 1.5x,
`JSON.stringify` from 1.2x to 1.0x, serializing a list of operations from 1.4x to 1.2x.

Left of it: a string of eight bytes or more is looked up in the string table (a tenth of the three
conversions now), and std's shortest digits of a double are still what `{:e}` writes.

In a loop of float arithmetic (`x = x * 1.0000001 + 0.25 / (i + 1)`, 2.7x) two fifths of the time were
not the arithmetic but getting to it: the operator found again by its kind in one function, then in a
second, and `Number::multiply` and `Number::divide` as the specification writes them, a step for each
special value (NaN, the infinities, the zeros), every step a question that read the operands again from
the heap. Those steps are the arithmetic of doubles, and so are the steps of `Number::remainder` what `%`
of two doubles gives: the three are now the operations themselves, as `Number::add` already was, and a
test holds `*`, `/` and `%` to the steps for every pair of the special values and of ordinary ones. The
four operators on two Numbers that are not both integers are computed where the instruction finds them,
and a double becomes a Value without a call. The loop went to 1.9x, trigonometry in a loop from 2.6x to
2.3x, rounding and clamping from 2.3x to 2.1x.

Reading or writing an element of a TypedArray asked whether the index is valid by the specification's
operations one after another: whether the buffer is detached, its length, whether the view is out of
bounds, the length of the view, each reading the view and its buffer again (and one assertion reading
the buffer a third time). It is one reading of the buffer and one of the view now. Arithmetic over a
`Float64Array` went from 4.1x to 3.3x, pixels written to a `Uint8ClampedArray` from 2.8x to 2.6x; typed
arrays from 2.8x to 2.5x, all cases from 1.68x to 1.67x.

Left: a double that is not an integer and has more than 45 bits of mantissa is a Number of the heap, one
allocated for the result of every operator, the intermediate ones too (a tenth of the loop, and the
collector's sweep of them a twelfth). A Value has seven bytes for what it holds; doubles in a Value
would take another layout of Value.

### 7. Building strings

Every string is interned: creating one hashed all of it. Appending in a loop therefore hashed the whole string on
every step, and then copied it: quadratic twice over. 160,000 appends took 12 s against 5 ms.

Done. The string table's hash is taken in 64-byte blocks, and a long string made by a concatenation keeps the
state of its hash, so appending continues from there. And an appended string is kept as shared pieces (the text
before, and what was added) that are joined only when something reads the text; appending to it shares all of the
pieces and adds one. The appended bytes are counted against the heap limit when they are added, a join is counted
when it happens, and a join that would take the heap past the limit ends the script as any allocation there does.
Appending is linear: 160,000 appends take 24 ms, and the long append cases were 2.5-4.2x then (from 170-300x).

By line, a step of such a loop was then mostly allocation: a copy of what was appended, a piece for it with its
bytes apart, a merged piece (and one more for each further piece it took in), and the record of the new text.
Now the text keeps its last bytes itself, up to 31 of them, next to its pieces: a step that appends a few bytes
copies them after those of the steps before and allocates the record alone, and a piece is made when they no
longer fit. What a piece takes in is found before it is allocated, so it is allocated once; what is appended is
read where it is. The six cases that append short strings are 21-36% faster (a character at a time 4.0x to
2.6x, short strings 3.9x to 2.9x, numbers 3.2x to 2.5x, text outside ASCII 2.4x to 1.7x, `fromCharCode` 3.2x
to 2.5x, escaping HTML a character at a time 2.3x to 1.7x); the group went from 2.2x to 2.0x.

A piece is also at least twice as long as the one after it now. It only had to be longer: a text appended to
with ever shorter strings had as many pieces as steps, and letting go of it went as deep. A text has no more
pieces than its length has bits.

Rows of HTML appended to one string (5.2x then) read 8% faster in the suite and 9% slower by their best run
after that change, and what was written here took the larger string table between two collections for the
reason. It was not that. A run of the case builds the text of the run before again, and so does a widget that
renders what it rendered before: every step makes the string the same step made last time, the string table
finds it, and to compare the two it joined both, the whole text copied twice for each thing appended. How
often that happened went by how many of the last run's strings a collection had left in the table, so by
how often the collector ran. An appended text is compared by its pieces now, with a buffer or with another
appended text, and nothing is joined for it; texts that continue one text share its pieces, and from where
both come to the same piece the rest is the same unlooked at, so the step that makes again what it made
before compares what it added. The rows of HTML went from 0.39 to 0.20 ms (2.7x), escaping HTML a character
at a time from 1.7x to 1.5x.

Sampled over the eight application cases together, a short concatenation was the largest thing to be named:
two fifths of the canvas command buffer, where each command makes `"rgb(" + r + ",10," + b + ")"`. A String
and a Number went the general way of `+` (the Vm put aside, both operands made primitives again), and every
concatenation allocated the text before asking the string table for it. A String and any primitive that
ToString takes are joined where they stand now (a Symbol and an object still go the general way), and two
strings that make at most 64 bytes are put together on the native stack and the table asked: the text is
copied to the heap only where no such string exists. The command buffer went from 3.5x to 2.7x, strings that
are made and dropped from 4.3x to 3.3x, rows of HTML joined from an array from 4.1x to 3.3x; the application
cases from 2.5x to 2.3x.

What was left then: every string made was hashed for the table with SipHash, keyed so that a script cannot
choose colliding strings, and prepending (`s = x + s`, 38x) copied and hashed the whole string every time,
5 KB a step on average in the case measured. By profile nine tenths of that case were SipHash, and in a
loop that appends a character a sixth: the bytes since the last 64-byte block hashed again at every step,
the state of the hash copied, and its four closing rounds.

SipHash was not slow (5 GB/s here). It cannot be continued from the other end, or put together from two
hashes. The hash of a text is now a polynomial of its bytes at a random point modulo 2^61 - 1
(`hashing.rs`, `TextHasher`): each byte plus one times a power of the point, taken eight bytes at a time
from a table of 8 by 256 products and 32 bytes to one reduction. The hash of two texts put together is the
left one's times the point to the right one's length, plus the right one's; so appending continues from
the hash of what was there by the bytes added, and putting text before costs one multiplication, where the
text keeps that power from the step before. Every string's record keeps the hash, so the table never reads
a text again to grow. The table takes a one-to-one mixing of the hash under a second random key. Two
different texts have the same hash for at most as many points as the longer has bytes, of 2^61, whatever
the texts; what was given up against SipHash is in `safe/SECURITY.md`.

A text that is being built keeps pieces at its start as it does at its end, with the same rule for how
long they are, and its first 31 bytes next to its last: text put before a string that a concatenation made
shares all of that string, as text appended does, and two built texts are compared side by side, each
side from its edge inwards until both walks stand at the same piece. And a concatenation of two strings
has its hash before it has its text, so it asks the table first and copies only where there is no such
string yet, whatever the length (it was so up to 64 bytes).

Prepending went from 38x to 1.9x (5.6 ms to 0.27 ms). The cases that append: a character at a time 2.6x to
1.9x, short strings 2.6x to 1.7x, numbers 2.2x to 1.8x, text outside ASCII 1.5x to 1.0x, rows of HTML 2.6x
to 2.0x, `fromCharCode` 2.3x to 1.9x, escaping HTML a character at a time 1.5x to 1.2x. The group went from
2.0x to 1.6x, all cases from 1.88x to 1.78x.

One thing on the way: the power of the point for a text's length, found by its bits at every step, was two
fifths of prepending as first written (2.8x). And an append of a few bytes to a long text asked the
allocator's count whether they fit, at every step; it is let through as a short string is, and the poll
holds the heap to the limit.

Left in a step of such a loop: the record of the new text allocated and, where the text was built before,
dropped again after the table found the old one (an eighth each for the table and the record), and the
collector's sweep of the records (an eighth).

`"row-" + i + "-" + (i & 15)` made a String at every `+`: three for one that is wanted, each hashed,
looked up in the table, allocated and collected (`gc/string-garbage`, 3.3x: 210 ns for the expression
against 63). A chain of additions is now compiled as one concatenation from its first operand that is a
String whatever is evaluated (a string or template literal): what stands before that operand is added up
as it is written, each operand after it is made the String it is there (ToPrimitive, then ToString) by an
instruction of its own before the next operand is evaluated, as the operators make it, and one
instruction puts the Strings together. Up to 64 bytes they are put together on the native stack and the
table asked, so the text goes to the heap only where it has no such String yet. A String among them that
is being built (areas above) is not copied: what stands before it and after it is put together and then
put before it and appended to it, as the operators did, so a loop that builds a text with such a chain
still adds to the text. Template literals are compiled the same way, with ToString for each
substitution.

Short strings made and dropped went from 3.3x to 1.5x, the canvas command buffer (`"rgb(" + r + ",10," +
b + ")"`) from 2.4x to 1.7x, rows of HTML from 2.1x to 1.5x, template literals from 1.6x to 1.4x; the
application cases from 1.9x to 1.7x, all cases from 1.71x to 1.68x.

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

`a < b` of two Strings left the loop of instructions for the general comparison, and that decoded both
texts into UTF-16 code units one code point at a time to compare them. The four comparisons of two
Strings are answered in the loop now, and the texts are compared as the bytes they are kept as: the
common beginning is passed over eight bytes at a time, and the first bytes that differ decide when both
are below 0xED, where the order of WTF-8 bytes is that of code units (it is not only where a surrogate
pair meets U+E000 and up, and those are decoded as before; a test compares every pair of a set of texts
around that boundary with the order of their code units). `string/equality-and-compare` went from 2.15x
to 1.8x, sorting objects by a number and then a name from 1.85x to 1.6x.

Replacing by a RegExp was the one built-in that stood out in the application cases (escaping HTML: a fifth of
the list that is rendered to HTML). `RegExp.prototype[Symbol.replace]` is written against the RegExp's
properties: it read `flags` through nine accessors, looked `exec` up for every match, and had each result made
an Array of, which it then read the match, the index and the captures back out of. Where `exec`, `flags` and
the accessors that `flags` reads are the built-in ones, found with nothing on the way that runs a script, those
steps read what the internal slots say: the matches are collected as ranges by the matcher alone and the result
is put together from them, a function still being called for each match after all were found. A sticky RegExp,
a lastIndex that is not a number, named groups and any replaced part of the protocol take the steps as written.
Replacing with a function went from 1.5x to 0.9x, with a string and a RegExp in turn from 30 to 10 ms (0.5x),
the rendered list from 2.1x to 1.7x. `match`, `matchAll`, `split` and `search` still go by the properties.

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
- A RegExp literal evaluated to the one RegExp it was compiled to, every time: a function that returned `/a/g`
  gave every caller the same object, with the properties set on it and the lastIndex that `test` and `exec`
  leave (`function f(s) { return /a/g.test(s); }` was true, false, true for one string). Upstream; two tests
  of test262 failed for it. Each evaluation makes a RegExp now, and all of one literal share the compiled
  matcher. Fixed.
- A value assigned to `lastIndex` that is not an index (negative, a fraction, a string, an object, 2^32 or
  more) was dropped where the RegExp had no other own property: it read back as undefined. A condition was
  the wrong way round. Upstream; 24 tests of test262 failed for it. Fixed.
- Two cases stayed 7-8% slower over the round that changed the step and the reading of small Numbers
  (`array/sort-with-comparator` 12.6 to 13.6 ms, `array/join-numbers` 6.1 to 6.5 ms). Built side by side with
  and without the change to small Numbers, the first is 2.5% apart and the second within what two runs differ
  by, and both builds spend their time in the same functions. Taken for placement, as above.
- The default of an element of a pattern could not read a name that an earlier element of the same pattern
  bound: `function f({ p, q = p + 1 }) {}` and `let [a, b = a] = list` threw a ReferenceError wherever the
  names are kept on the stack, which is in every function whose closures do not capture them. The compiler
  takes a stack variable read inside its own declaration for uninitialized, and the declaration of a name in
  a pattern is the whole pattern. Upstream; test262 has no test that fails for it. It is refused now only
  where the read comes first (an earlier element, the element's own default, key or value, or what the
  pattern is initialized from). Fixed.
- The constructor, the methods, the getters and the setters of a class took the mode of the code that
  made them: in a script that is not strict (which a widget's is), an assignment to a name that is not
  declared made a global, a write that fails was dropped, and `this` was the global object in a method
  called without one. All of a class is strict mode code. Upstream; nine tests of test262 failed for it and
  pass now. The functions that are elements of a class are strict now; what runs in the context of the code
  around the class (static blocks, the initializers of static fields, computed keys, the heritage) is not.
  Fixed for the functions.
- An Array, and any object that has no properties of its own yet, is asked for its prototype through the
  running realm, not its own: `g.eval("[1]")` of another realm `g` has the caller's %Array.prototype% as
  its prototype, and `x instanceof g.Array` is true for it only because the call of g's
  Function.prototype[Symbol.hasInstance] makes g the running realm while it looks. Upstream. Not fixed;
  the shortcut for instanceof is taken only for the running realm's method because of it.
- Iterating an Array panicked (the engine died) where a "return" or a "throw" method could be found from
  an Array Iterator: `Object.prototype.return = function () {}; var [a] = [1, 2];`, or a `break` out of
  `for`-`of` over an Array after that; and `yield*` over an Array thrown into, after
  `Array.prototype.throw = …`. Both were left as `todo!()`, and both were looked for on %Array.prototype%,
  which an Array Iterator does not inherit from, so a "return" put on %ArrayIteratorPrototype% was never
  called. Upstream; test262 has no test that fails for it. Where one of them has to be looked for with the
  iterator, the walk is given the Array Iterator object it stands for, at the element it has come to, and
  goes on as any iteration does. Fixed.
- A "next" put on %ArrayIteratorPrototype% in place of the original was not called by anything that
  iterates an Array with the original `Symbol.iterator` method: patterns, `for`-`of`, spread, `yield*`,
  and the constructors of Set, Map and WeakSet and Object.fromEntries, which read the elements
  themselves. Upstream; test262 has no test that fails for it. They all ask now whether "next" is the
  original, as GetIterator would read it. Fixed.
- Object.fromEntries looked for the iterator method of an Array on %Array.prototype%, not on the Array:
  one with a `Symbol.iterator` of its own, or of a subclass, was read element by element all the same.
  Upstream. Fixed.
- An element of an array pattern with a default value skipped an element of the array where the one it
  was to read was a hole that a getter of a prototype stands in for, or an accessor:
  `var [a, b = 0, c] = [1, , 3]` with a getter for 1 on %Array.prototype% gave 1, 3, undefined. The step
  that runs nothing had moved on before it found that something must run. Upstream. Fixed.
- A pattern that ends before its iterator does closes the iterator, and did it wrong three ways: "return"
  was called with undefined as an argument where it gets none; what it gave back was not checked for
  being an object (a TypeError); and where it threw, it was called a second time by the handler of that
  error. Any iterator, not only an Array's. Upstream; test262 has no test that fails for it. Fixed.
- `string/index-of` moved by a third again with the build of the shared stacks (2.2 to 2.9 ms, 0.5x to
  0.7x), and not with the build of the commit before it: the search has not changed since it last did
  that. Placement, as above. (And back to 2.2 ms with the build of the round after.)
- Whether an Array takes new properties and whether its "length" is writable were one flag.
  `Object.preventExtensions(a)` made "length" read-only: `a.length = 1` did nothing, `a.pop()` threw, and
  a sealed Array reported a read-only "length". And an Array whose "length" was made read-only took no
  element in a hole under that length (`a[1] = 2`, defineProperty, `fill`). Upstream; four tests of
  test262 failed for it and pass now. The flag is the writability of "length" alone now, and the
  Array's backing object says whether it is extensible, as it does for every other object. A template
  object and its raw strings, which were frozen by way of the flag, are given a read-only "length"
  themselves. `unshift` took its direct way without asking whether a prototype has a setter under an
  index; it asks as `push` does. Fixed.
- A Proxy whose target is a function is not accepted where a callback is asked for:
  `[1].map(new Proxy(function () {}, {}))` throws a TypeError (IsCallable is false for it), `new` of
  one throws and `x instanceof` one is false; `Proxy.revocable` is not there at all. Upstream.
  Not fixed.
- `Object.isFrozen(Object.preventExtensions([]))` is false here and true in V8, whose "length" for that
  Array is writable all the same. Taken for V8's departure, and left out of the comparison with Node.
- One run of the widget sandbox's tests in hosted mode failed a test that bounds a widget by its layout
  queries: the widget was stopped for 200 ms of continuous execution after 19 queries instead. The machine
  was under a load of ninety at the time; three runs on a quiet machine passed. The limit is wall-clock time.
- One run of `limits_tests` in fourteen had one failing test (which one was not captured); the thirteen other
  runs, eight of them of that file alone, passed. Its tests end scripts by wall-clock deadlines. Not explained.

- The joins of the texts that the string table compared (area 7) were counted against what may be joined
  between two collections, and a join past that ends the engine as out of memory. With the estimate the
  collector ran often enough to hide it. On top of the Android line, which counts what the engine allocates
  and collects less often for it, the rows of HTML ended the engine on their thirteenth run and the escaping
  of HTML on its second, with 6 MB in use under a limit of 64 MB: the benchmark found it after the two lines
  were joined, and no test had. Fixed with the comparison, with a test that runs both under the count.
- Joining the two lines brought their checks together, and reading the Android line's changes for it found
  defects in what both share (`safe/SECURITY.md`, "How it was tested"): a process that `JSON.stringify` of a
  Proxy with a made-up length ended, native loops the interrupt did not reach, a RegExp search's stacks
  outside the memory limit, a host function's arguments copied without asking it, and DataView bounds that
  wrapped around on a 32-bit target. Fixed, each with its test.
- A fix on one line for a function the other had copied does not conflict: the Android line made `Math.max`
  take +0 over -0 without `f64::max` (which answers -0 on 32-bit ARM), and the copy of that fold which runs
  `Math.max` in place of a call kept the old line through the rebase. Found by comparing every commit before
  and after; the copy has the fix.

- A function declared in a block and named `arguments` does not take the place of the arguments object
  after the block in sloppy code (Annex B.3.3), here or in the build this started from; Node does it.
  Found by the script written for the arguments object's stack slot. Not fixed.

- The shorter way of an object spread (`{ ...source }` as a copy of the source's shape and values) had
  never run, here or in the build this started from, and would have made broken objects if it had: it was
  asked of the source with the source for its argument, so the target was never empty in its eyes; it
  took the target's shape for the source's; and it let through properties that are not writable or not
  configurable, whose copies are both. Found by reading it for the profile of the spread. Written again,
  with a script of fifty-five checks against Node as its test.
- `Object.prototype.__proto__` is not an accessor here, nor in the build this started from: `in` and a
  read find `__proto__` on no object, and `Object.assign({}, JSON.parse('{"__proto__": {}}'))` makes an
  own property of that name where Node sets the prototype. Annex B.2.2.1; not added.
- `String.prototype.repeat` made each lone surrogate of the text U+FFFD, and did not make a pair of a
  trail surrogate at the start with a lead surrogate at the end of the copy before; so here and in the
  build this started from. Found by the script written for the functions run in place; fixed, with that
  script as its test.
- A template literal evaluated all of its substitutions and then converted them to Strings, where each is
  to be converted before the next is evaluated: a `toString` that the next expression can observe ran
  late, and one that throws did not keep the later substitutions from being evaluated. So here and in the
  build this started from. Found while reading how concatenation is compiled; fixed with the chains of
  `+`, which convert each operand in its turn, and tested with them.

- `loop/switch` read 4% slower after the round of the text of a Number (5.5 to 5.75 ms, in seven runs of
  each binary in turn), which touched nothing it runs: where the loop of instructions lands in the binary
  moved. It is 0.9x of QuickJS. Not followed up.
- The length of `Function.prototype[Symbol.hasInstance]` was 0 where it is 1. Upstream, and one of the
  expected failures of test262; fixed while that method was read for `instanceof`, and the expectation
  removed.
- The comparison of Strings, as first written, made the loops that compare no String slower: nested
  `while` loops by 16%, a sum of integers by 6%, a `switch` and a closure call by 7% each. It was a call
  put in the branch for "not two Numbers" of two helpers that are compiled into every instruction of
  arithmetic and comparison, and each copy of them was the worse for it. Every test passed; the table of
  the whole benchmark showed it, case by case against the round before, and the two binaries run in turn
  confirmed it. One function out of the loop now has all that is not two integers, and the call for
  Strings is compiled only into the four comparisons. A change to those helpers is measured on the plain
  loops first, whatever case it was made for.
- `with (object) { name }` does not ask the object for the name: a name the object has is read from the
  scope outside it, here and in the build this started from. And a function declared in a block is not
  a variable of the function around the block after the block has run (Annex B.3.3): `typeof` of it
  stays "undefined" there. Both upstream, both seen in the script for the variables kept for a call,
  where Node gave what the specification says; neither is fixed, and the script leaves them out.
- Private names put on an intrinsic object moved every property of the object under its shape, which
  stays the same shape, and no property lookup cache was told: a constructor that returns the object to
  a class with private fields (`class S extends B { #x }`, `B` returning `globalThis` or
  `Array.prototype`) left every cached read of that object's properties, its own and those inherited
  through it, reading what had moved into the place. The script that does it to the global object ended
  with "not a function" at the next global it called, here and in the build this started from. Upstream;
  test262 has no test that fails for it. Fixed: every cache forgets what it found when that happens, once
  for an object. Found by the script written for the globals, which is its test.
- The same again a round later, by another way: an arm for `yield` among the plain instructions, which
  no loop but a generator's runs, made `loop/switch` 14% slower and nested `while` loops 4% (the two
  binaries run in turn; every test passed, and the mean of the benchmark had improved). Nothing those
  loops run was changed: the compiler laid the loop of instructions out otherwise. Without the arm a
  `yield` takes the general way of an instruction, which costs a generator 1-2%, and the loops are where
  they were. In the same binary a plain call reads 4-8% faster than the round before (`call/plain-function`,
  `call/closure`, `prop/getter-setter`), with nothing in its path changed but that a new frame is made by
  a function of its own; whether that or the layout did it is not established.

## Checking a change

`cargo test -p nova_vm -p nova_embed -p nova_embed_ffi -p wtf8`, the widget sandbox's own suite against the built
library (224 tests in process, over the stream and hosted, the attack corpus, the compat corpus), then
`safe/bench/run.py --baseline …` for the numbers, and test262 as `safe/tools/gate.sh` runs it (no unexpected
result; 13 or 14 tests that timed out now pass, depending on the run).
