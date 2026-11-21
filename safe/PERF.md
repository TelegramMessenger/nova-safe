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

One process times a case for 300 ms, and where it lands in memory moves its time by several percent: two tables
of one library, one right after the other, differ by 5% and more on single cases of either engine, and by two or
three in the count of cases over 2x. `--repeat N` runs every case in N processes and keeps the fastest; the tables
that say where things stand are taken with five. A change of a percent or two is not read from tables at all: the two
binaries run one case in turn, fifteen times each, and the fastest runs are compared (a case that two tables had
6-9% slower after a change of 15 instructions elsewhere was within 0.8% so).

Where the time goes inside the interpreter loop does not show in a profile by function: the loop is one function.
`xctrace record --template 'Time Profiler'` and `xctrace export` give the sampled addresses, and those against the
disassembly give the time per machine instruction. That is how the loop's own share was found (area 1).

### Where a function lands

A case that runs none of the code a change touched can still move by 3-5%, by where the change left
the loop of instructions in the binary. Round after round the same few cases did (a step of the
particles, `loop/switch`, `loop/float-math`), up in one build and down in the next, with `Vm::run`
the same instructions each time.

It is not where the function starts in a 64-byte cache line. For a try every function was put on a
multiple of 64 bytes (`-C llvm-args=-align-all-functions=6`; the binary 1.8% larger). The substring
change of area 8, which none of those cases runs, still read 2.0-3.7% slower on the three with both
sides built so, and one commit built with and without it was within 1% on nine of ten such cases
and 2.5% slower on the particles. Not kept.

It is the address itself. With `Vm::run` linked first in both binaries, so that it is at the same
address in each, the three read -0.9%, -0.2% and +1.7%; with the dozen functions it calls most
linked in front of it, at the same addresses too, the seven cases looked at are between -1.4% and
+0.8%, which is what two runs of one binary differ by. (No instruction of those functions differs
between the two builds; what a processor keeps under the address of a branch is presumably what
does.)

So the benchmark binary has those functions at its start: `safe/bench/order.txt` names them and
`run.py` gives the linker their order (macOS). A change that leaves them as they are leaves them
where they are, and what moves in a comparison is what the change did. The interpreter of QuickJS and
a dozen functions around it are first, so that nothing in Nova moves them, and the loop is next, so
that a change to another function of Nova's leaves it where it is. (It was last of them at first, so
that a change to it moved none of the others. But the others change more often than it does, and the
first of them that grew, by 264 bytes, moved the loop: a `switch` in a loop read 5.6% slower in the
table for a change that runs none of it, and 0.1% with the loop linked where it had been.) And each
of them is filled up to the next 4 KB from the first, by a symbol of nothing after it that `run.py`
makes once it knows how long the function is (the binary is linked twice): a function that grows
or shrinks within its slot moves none of the others, and the rest of the binary starts where the
last slot ends. Having them there costs nothing by itself: one commit
linked both ways reads 0.03% apart on the panel of 34 cases, none of them 2% worse.

This is a property of the benchmark's binary, as the old placing was. A product build has these
functions wherever its link puts them, a few percent either way on the cases that feel it; an
embedder can give its linker the same order. And it does not settle a change inside `Vm::run`, which
still moves everything after it within the function: two builds that differ so are still compared
on the panel (area 5).

### What an operation costs

A timing says that a case is slow and a profile where its time goes, by function; neither says what one
call or one property read costs next to the other engine's, and neither can be taken while the machine
does something else. The instructions a process retires can: `/usr/bin/time -l` reports them (macOS).
What the script itself retires is the same from run to run under any load. The count of a process has
the kernel's work for it in it too (its pages), which varies by some millions of instructions, so
every case is run three times and the least count taken: two such lists of one binary then agree to
0.3 instructions per operation.

```bash
safe/bench/prices.py [--bench target/bench/work/bench] [--filter call/] [--runs 3] [--save now.json] [--against before.json]
```

It runs the cases of `safe/bench/prices.js` in each engine, in the runner that `run.py` built. Each is
a loop of a million turns, and a case named `x@y` is the case `y` with one statement changed (one
operation more in a turn, or one in place of the cheapest of its kind), so the two processes differ by
a million times the price of the operation. As of the round of the shapes that `new` keeps
(area 3), in machine instructions per operation:

| operation | QuickJS | Nova | |
| --- | ---: | ---: | --- |
| call, no argument / one / two | 228 / 270 / 262 | 478 / 561 / 569 | 2.1-2.2x |
| call of a method, of one that reads `this.n`, of an inherited one | 312 / 417 / 328 | 697 / 781 / 739 | 1.9-2.3x |
| a callee that reads a variable of its maker, in place of its parameter | 2 | 43 | |
| a callee that assigns one, more | 25 | 109 | |
| read of a variable of the enclosing function, in place of a local | 2 | 2 | |
| assignment to one, in place of a local | 3 | 49 | |
| read of a global, in place of a local | 41 | 60 | 1.5x |
| assignment to a global, in place of a local | 61 | 163 | 2.7x |
| property read, own / inherited, in place of a local | 53 / 68 | 100 / 144 | 1.9-2.1x |
| a second property of the same object, in place of a local | 53 | 75 | 1.4x |
| property assignment, in place of a local | 53 | 119 | 2.2x |
| getter / setter | 328 / 405 | 726 / 762 | 2.2x / 1.9x |
| integer `+` of two locals, more | 24 | 88 | 3.7x |
| `-(i & 7)` / `-x` of a double, more | 23 / 9 | 121 / 111 | |
| `x * 0.999 + 0.25` on doubles, in place of a constant | 68 | 221 | 3.3x |
| `Math.floor(i / 3)` / `Math.sqrt(i)` / `Math.sin(i)` | 262 / 253 / 326 | 495 / 434 / 511 | 1.6-1.9x |
| `Math.max(i, 7)` | 271 | 373 | 1.4x |
| Array element read / written | 56 / 53 | 134 / 169 | 2.4x / 3.2x |
| Uint8Array element read / written | 89 / 78 | 162 / 172 | 1.8x / 2.2x |
| Float64Array element read / written | 106 / 76 | 231 / 172 | 2.2x / 2.3x |
| `new` of a function / of a class, one field set | 1073 / 1129 | 1840 / 1812 | 1.7x / 1.6x |
| an object literal of two properties | 745 | 490 | 0.66x |
| one callback of `forEach` (the whole turn) | 395 | 742 | 1.9x |
| one step of `for-of` over a generator (the whole turn) | 491 | 911 | 1.9x |
| `s.charCodeAt(i)` | 350 | 471 | 1.3x |

Instructions are not time. Nova runs more of them in a cycle than QuickJS does on this machine (over
the benchmark's worst cases it retires 2.2 to 3.0 times QuickJS's instructions and takes 1.8 to 2.8
times its time; a loop that adds integers retires 1.9 times as many and takes the same time), so
the table of `run.py` still decides what is kept. What the prices say is where a path is long: the
arithmetic of doubles was first (371 for the two operators of the table before that round; each
result is still a Number of the heap, area 6), then the globals, then the calls, which were 580 to
970 instructions where QuickJS has 230 to 420 and are spread thin (entering the function 18% of the
time of `call/closure`, leaving it 11%, taking up the code that runs next 15%). And they say what a
profile cannot: that a function was fifteen instructions longer than its work, at every call of it,
for a frame it had no use for (area 1).

Two builds are compared the same way, case by case of the benchmark: the instructions of a process
over the runs its case reports. A change that makes a path longer shows there exactly, with the machine
busy, before any timing can be trusted; that is how the loss of the calls in the try at the loop's
step (area 1) was found, which four placings of the loop had only shown to be real.

## Where it stands

Nova's time over QuickJS's, geometric mean per group, on an Apple M-series machine (QuickJS fork `fbc91d8`, `-O2`;
Nova release with fat LTO, rustc 1.96.1). "Before" is `c13a0dfc` with the embedding fixes; "now" has the changes
below.

| Group | Cases | Before | Now |
|---|---|---|---|
| typed | 4 | 10.6x | 1.5x |
| call | 14 | 10.4x | 1.5x |
| math | 4 | 9.7x | 1.5x |
| string | 26 | 10.6x | 1.2x |
| prop | 13 | 7.1x | 1.2x |
| map, set | 4 | 4.6x | 1.1x |
| app | 8 | 6.1x | 1.4x |
| array | 14 | 5.8x | 1.2x |
| regexp | 4 | 2.4x | 1.3x |
| loop | 11 | 11.1x | 1.1x |
| gc | 3 of 5 | 3.6x | 0.9x |
| json | 2 | 1.0x | 0.8x |
| all | 108 | 7.6x | 1.23x |

The worst case was 303x (appending to a string 100,000 times); that one is 1.6x, and prepending, which
was the worst after it (38x, area 7), 1.7x. The worst is now the call of a closure (1.97x), and no
case is over 2x: of the 107 that both engines run, 26 are at or under QuickJS's time and 40 over
1.5x ("now" is the tables of a quiet machine at the end of this section).
Over all cases it is 1.23x: 2.12x before the round of patterns, Array iteration and
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
machine with other builds, and each engine's times read 1% higher than in the run before). A return
that cuts back only what the function left, and a last instruction read as any other, took 3-8% off
the calls and the mean to 1.519x. The `length` of a TypedArray remembered to be its accessor took a
loop over a `Uint8Array` from 2.0x to 1.2x, and the mean to 1.503x; the getter and the setter of a
property remembered likewise took a tenth off a loop that calls both, with the mean where it was
(1.505x in that run). An Array of Strings and Numbers joined from where they are took the mean to
1.488x, and the properties of functions read by their caches to 1.473x. `typeof` done in the loop
and an object for `new` made with its prototype at once left the mean there (1.48x in a run where
QuickJS's own times were 0.9% lower). What a Map, a Set, a RegExp and a TypedArray inherit read by
their caches took it to 1.47x (the run of that round was taken while other work ran on the machine,
and was repeated from the round's saved library when the machine was quiet), a table of
descriptors that knows which storages have any to 1.46x, and the `+` of Strings done where the
operators on Numbers are done to 1.44x. What a RegExp match gives made without defining it piece by
piece took the two RegExp cases that were over 2x under it (the mean read 1.44x again, in a run
where QuickJS's own times were 2% lower), and an Array's storage looked at once to write an element
took the loop over the elements of an Array under it (1.45x, QuickJS's times lower again). The step
of the iterator of a String, a Map or a Set taken without its result took the mean to 1.43x, and
less asked at every `push` and at every element of a `join` took the building of a text from pieces
under 2x. The descriptors of arguments objects kept once for all of them took the case that reads
`arguments` under 2x as well (1.42x, with 92 cases at or under 2x: the run of that round shared the
machine with another build, in which both engines' times read 2% higher and single cases up to a
tenth, and was repeated from the round's saved library when the machine was quiet). The capacity of
a storage kept with a cached property of the object's own took 7% off the two cases that read and
write properties most (a step of the particles 2.75x to 2.5-2.6x, the properties of one object
2.4x to 2.3x) and two more cases under 2x, with the mean at 1.41x (taken the same way: the machine was busy
when the round's checks ran). A substring looked up where it lies and a short text counted for its
length took the slicing of a text that is not ASCII under 2x, with the mean at 1.40x. From that run
on the benchmark's binary has the hot functions of both engines at its start ("Where a function
lands"), which moved QuickJS's own times too: its calls into its built-ins read a tenth faster
(`math/trig-and-sqrt`, 2.1x to 2.4x with Nova's time where it was), and one run of the whole suite
still differs from the next by a few percent on single cases of either engine. What was last found
of where the elements of a TypedArray are, kept for the next element, took 3-4% off the loops over
typed arrays (`typed/pixels-rgba` to 2.0x, `typed/float64-math` to 2.2x), with the mean where it was
(1.40x, in a run where QuickJS's own times read 1.6% lower). The arithmetic of doubles worked out
straight through (area 6) and a global read as one slot (area 2) took a fifth off a loop of
arithmetic on doubles (`loop/float-math` 1.9x to 1.55x) and a tenth off what calls `Math` or keeps
doubles (`math/trig-and-sqrt` 2.4x to 2.1x, `typed/float64-math` 2.2x to 2.05x, a step of the
particles 2.65x to 2.5x, `math/floor-round-minmax` and `typed/pixels-rgba` under 2x), with the mean
at 1.396x from 1.401x and 97 cases at or under 2x from 95: both tables taken from the saved
libraries with every function of `order.txt` in a slot of its own ("Where a function lands"), where
the table of the round's checks had a `switch` in a loop 5.6% slower for no change to it. The native
stack asked about once for the calls the loop runs itself (area 3) took a percent off the cases
that call: 1.394x, in tables of the fastest of five processes for a case, where 97 cases are at or
under 2x and three more sit on it (a plain call, a call of `Math`, padding a text). The copies of a
text with no surrogate in it made as its bytes (area 8) and the functions of `Math` that read their
Number once (area 6) took two of those three off it (`string/pad-and-repeat` 1.98x to 1.86x,
`call/native-math` 2.02x to 1.91x, and `math/floor-round-minmax` 1.94x to 1.82x): 1.385x from
1.393x in two tables taken one after the other, with 99 cases at or under 2x and 20 at or under
QuickJS's time. The this value of a call found when the call asks for it, the return of a function
as part of the loop (area 3) and the functions the loop calls written without a frame (area 1) took
four more under 2x, the calls among them: a function that calls itself 2.18x to 1.93x, a method
2.11x to 1.93x and one inherited through three prototypes 2.15x to 1.95x, the arithmetic of a
Float64Array 2.07x to 1.96x, with a plain call at 1.74x from 1.96x and a closure at 2.47x from
2.66x. Over all cases 1.360x from 1.385x, 103 at or under 2x. The this value of a method call kept
on the stack under the function (area 3) took 5 to 8% off what calls methods: a method 1.95x to
1.81x and one inherited through three prototypes 1.98x to 1.85x, calls of `Math` 1.92x to 1.76x and
trigonometry under 2x (2.04x to 1.96x), a Map and a Set by their methods 8 to 10% less (the Set at
QuickJS's time), `push` and `pop` 8%. Two cases read slower by where the loop's code lay: the
properties of one object 2.16x to 2.23x and a step of a generator 2.02x to 2.09x. Over all cases
1.355x from 1.365x in two tables taken one after the other, 104 at or under 2x. What the arms hand
on (area 1: the tables not written for a call that is not made, a property's value taken as it is,
the name of a variable kept beside its place) gave those two back what they had lost and took 4% off
the cases that read properties and variables: a closure 2.40x to 2.30x, the properties of one
object 2.19x to 2.11x, a step of a generator 2.05x to 2.02x, trigonometry 1.96x to 1.88x. Over all
cases 1.336x from 1.353x, with the same four over 2x and 22 at or under QuickJS's time. A generator
that is suspended at its `yield` by the loop that resumed it, and hands over the one handler and
the one iterator it has as the entries they are (area 5a), took a step of a generator off 2x
(2.04x to 1.84x): 1.333x from 1.340x, with three cases over 2x. Every byte made the kind of an
instruction (area 1) took 5.7% off the properties of one object read and written, which is under
2x with it (2.09x to 1.98x), and 1 to 3% off the calls and off the loops that read elements:
1.328x from 1.333x, with two cases over 2x. The second property of a kept name read by one
instruction (area 4) took 7.1% more off that case (2.04x to 1.89x in two tables taken one after
the other) and left the mean where it was: 1.327x for 1.324x, in a pair where QuickJS's own times
read 1.2% lower and Nova's 1.0%. A `switch` in a loop and a step of the particles read 3 to 6%
more time in that pair with the instructions they had before, by where the loop's code lies. An
instruction read with every argument it can have in one look at the code (area 1) took 2 to 8% off
the loops and the calls: 1.309x from 1.319x in two tables taken one after the other, with the same
two cases over 2x (a step of the particles 2.3x, a closure 2.25x for 2.29x). Two RegExp cases read
12% and 21% more time in the second of those tables, which were taken while other work ran on the
machine, and 3.1% more and 0.5% less with the two binaries run in turn. What a property read
finds written whole into the result register (area 4), and a value compared with a constant by one
instruction (area 1), are counted and not timed: other work had the machine while their checks ran
and after. By the instructions they retire, the properties of one object are 3.8% fewer with the
first, and a step of the particles 3.0% and then 6.2% fewer. The tables of their checks, taken
under that work, have the whole at 1.294x and 1.293x and the same two cases over 2x (a step of the
particles 2.15x, a closure 2.25x); the table above is the one before them. A property stepped by
one instruction (area 4) took 3.35% more of its instructions off a step of the particles, counted
the same way, a property that is pushed by the instruction that reads it 1.5% more, and an
assignment that takes its object from the variable's slot 3.4% more, for 0.5 to 0.9% more
instructions in the cases that call. An assignment to a variable of an enclosing function that
leaves its value for the read after it (area 2) took 7.4% of its instructions off the call of a
closure, and an operator that reads its left operand from a local's slot (area 1) 2.7% more.

Where that leaves the two cases, with no table of a quiet machine yet: the two engines run in turn
in one binary, and the least of nine to thirteen runs of each taken, which other work on the
machine leaves near what a quiet machine gives (QuickJS's least times are those of its quiet
tables). A step of the particles is 1.84 to 1.92x that way, and the call of a closure 2.04x, and
2.01 to 2.02x once the names of bindings are compared at once, 1.99 to 2.00x once the read of
such a variable asks nothing twice, and 1.96x once a place in the code keeps its name as the
bytes that are compared (area 2). By this measure no case is over 2x: the cases that the last
table had nearest to it are, on that build, the call of a closure 1.96x, trigonometry 1.91 to
1.97x, the arithmetic of a Float64Array 1.87x, objects spread and assigned 1.86x, the properties
of one object 1.83x, a step of the particles 1.81x, a sort with a comparator 1.81x, and the rest
1.79x and under. (The tables of a quiet machine, at the end of this section, say the same.) The
properties of one object are 1.82x, trigonometry 1.91x, the arithmetic of a Float64Array 1.88x, a
step of a generator 1.81x, a plain call 1.72x.

With the global scope saying when it changes and the sweep by the words of the marks (areas 2 and
9), by the same measure: trigonometry 1.71x, calls of `Math` functions 1.52x for 1.68x, a loop
over a global 1.55x for 1.76x, the arithmetic of a Float64Array 1.79x. What that round did not
touch reads as it did or within what the measure scatters by under more of the other work (a load
of 50 to 120): the call of a closure 1.96x, the properties of one object 1.79x, a sort with a
comparator 1.86x, a step of the particles 1.90x, objects spread and assigned 1.90 to 2.07x on this
build and 1.90x on the one before in the same minutes, where the two builds run in turn are level
(34.4 to 35.8 ms each, whichever is run first) and retire the same instructions.

With the assignment to a global that asks the state once and makes no Reference (area 2), a loop
that adds to a global is 1.49x by that measure; the call of a closure reads 1.95x, a step of the
particles 1.85x, objects spread and assigned 1.90x.

`Object.assign` to an empty object by the shape of its source (area 4) took objects spread and
assigned from 1.90x to 0.90x. Looked for in the other cases nearest 2x and not found, a cost in one
place as that was: a sort with a comparator is the calls of the comparator (a fifth of its samples
fall on the instruction after each call returns, in the merge that does nothing else), and the
properties of one object and a step of the particles are the read and the write by the cache and the
loop of instructions, where a quarter of the samples are on the three instructions that fetch the
next one.

An element of a TypedArray read and written by one function (area 6): the arithmetic of a
Float64Array 1.75x, the pixel loop 1.87x, the fill of a Uint8Array 1.11x, by the same measure.

The shapes that `new` keeps (area 3): `new` of a class 1.50x, a derived class 1.62x, a tree built
and walked 1.30x. The call of a closure is where it was, 1.96x, and is the case nearest 2x.

The tables of a quiet machine, at last, after the rounds of an Array's plain elements, `typeof`,
the operators of one operand, the length of a String, ToBoolean and the three constants: two of
the whole suite from the last round's saved library, eight minutes apart, each the fastest of five
processes for a case. The wait for them had gone by the load average, which on this machine stays
near twenty while one process of the desktop spins in the kernel on four of its eighteen
processors, and says nothing: the processors were 70% idle all morning but for the builds and
checks of the rounds themselves. Taken while they were idle but for that process, over the 107
cases that both engines run: 1.226x and 1.227x, the worst 1.98x and 1.96x (the call of a closure).
After it a step of the particles 1.86x in both, a sort with a comparator 1.82x, and eleven more
between 1.7x and 1.8x (a function that calls itself, a method through three prototypes, a step of
a generator, the properties of one object, the pixel loop, an arrow as a callback, padding a text,
numbers appended to one, trigonometry, a slice of a text that is not ASCII, a method); 26 cases at
or under QuickJS's time. Two older saved libraries measured in the same quarter of an hour: the
one the table at the top of this section was last taken with reads 1.302x with the worst 2.35x (a
step of the particles), and the one from before the global scope said when it changes 1.289x with
the worst 1.95x. So what the two engines run in turn had said holds: no case is over 2x, and none
was at that earlier round either; the mean has come down by a twentieth since.

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

The step itself was tried shorter, and is as it was. Of its 22 instructions on the way that is always
taken, one is a jump over the other way: the read of an instruction where the code has less left than
the five bytes the loop reads at a time, which the compiler had made one with the usual read. With that
read in a function of its own (never read into the loop, and marked as rare) the step was 21 with
no jump taken, a `switch` in a loop 12-16% faster over four placings of the loop ("Where a function
lands") and the other loops 1-2%. Every case that calls was 2-5% slower, at every placing.

The instructions retired (above) said why, which no timing had: a callback ran 3.4% more of them. The
compiler now read the allocator into the arm that makes one more frame when a call finds none, and
looked up whose memory it counts (a thread-local of the embedding) where the function starts, at every
entry, to allocate once in a long while; and it worked out where the scoped collections of the running
code start (their count times their size, for a truncation that few instructions need) before the loop,
at every call and every return. Both went away with that rare work in functions of their own, and the
loop was 139 instructions shorter than before the try. But a function that returns nothing runs off the
end of its code, which is the read that had been taken out of line: 19 instructions more for every
callback, setter and constructor (the price list showed it at once: those three, and nothing else that
calls). With the end of the code asked for in the loop again, in one comparison, the compiler stopped
reading the entry of a call and a dozen small functions into the loop at all (the loop a sixth smaller,
every call 50 to 60 instructions longer), and with those marked to be read in always, every call was 90
longer. Dropped: four forms, four different loops. The loop of instructions is one function of 13,000
instructions whose every part the compiler decides again for any change to it, and the step cannot be
shortened by one branch without paying for it somewhere that only a count of instructions finds. What
would shorten it soundly is an instruction of its own at the end of every code (so that no read is ever
short, and the loop has one way to read), which is a change to the compiler of the bytecode; not done.

One test of the step did go, with no branch moved for it. The byte that an instruction starts with
names its kind, and 231 of the 256 values of a byte did: the read of every instruction asked whether
its byte names any, a comparison and a branch for what only a defect of the engine can bring about
(code is made by the compiler of the bytecode and by nothing else: a compiled script is never made
from bytes). Every byte is a kind now. The 25 that no instruction has are kinds that nothing emits
(`Unused231` to `Unused255`; a constant of the build checks the last of them to be 255), so the kind
of a byte is the byte, and reading it asks nothing. Such a byte in code fails as it did, with the
same message, where it is looked at with care: by whoever walks code (to print it, to name its
sites) and by the general way of an instruction, which is where the loop sends every kind it has no
arm for (`Instruction::is_unused`).

By the price list the turn of a loop is 4 instructions shorter, a call 13 to 17 (485 for 498 with
no argument), a method call 14, a callback of `forEach` and a step of a generator 18 each, `new` 13,
a getter 8 and a setter 6, an element of an Array 4 read or written and of a typed array 2 to 3,
the `+` of two integers 3, `s.charCodeAt(i)` 5, `Math.max` 6. A variable of the maker read by a
callee and a global are one longer each (58 for 57, 103 for 102) and nothing else is up. With the
two binaries run in turn, fifteen runs each: the properties of one object read and written take
5.0% less time, nested loops 4.1%, a sum of integers 3.7%, a function that calls itself 2.6%, a
method inherited through three prototypes 2.5%, the RGBA loop over a typed array 2.4%, calls of
`Math` 2.0%, a step of the particles 1.9%, a closure and a step of a generator 1.7% each, a method
1.5%; a plain call, arithmetic on doubles, a `switch` in a loop and the variables of enclosing
functions and of the global scope are within 1% of where they were, and an empty loop reads 3.3%
more with four instructions fewer in its turn, by where its code lies now.

The table of the arms still asks one thing of the kind: whether it is past the last arm the loop
has, since the kinds after that go the general way together. With an arm of its own for the last
kind, so that the table has an entry for every byte, that comparison went too: the turn of a loop
was one instruction shorter and every call three to five longer. Not kept.

And what an instruction has beyond its first two arguments. The comparisons of a local that jump,
the steps that end a loop, the reads of a property of a local or of a kept name and the reads of a
kept name have four arguments, nine bytes. The loop read the first two with the instruction, and
the arm fetched the others from the code by the place of the next instruction: is that place four
bytes into the code, is it within the code, where does the code start, the address, the load; eight
machine instructions for one argument, at every turn of a `for` loop and at every `p.x`. The loop
now reads an instruction with the eight bytes after it, the longest instruction there is, in its
one look at how much code is left (the padding after the code is eight bytes where it was four),
and hands that window to the arm: the third argument is one load from it. The compiler made more
of this than the arguments: with the window at hand the step from one instruction to the next is
shorter for every kind.

By the price list the turn of a loop is 214 instructions for 223 and a variable of the enclosing
function read in it 3 for 10 (QuickJS: 2); a call is 8 to 10 shorter (476 for 484 with no
argument) and a method call 12; a property read is 111 for 122, a second one of the same object 87
for 94 and an assignment 118 for 127; an element of an Array is 9 to 10 less read or written and
of a typed array 9 to 11; `new` 20, a step of a generator 14, `s.charCodeAt(i)` 12, a getter 11.
Nothing is longer. A widget takes the memory it took (the four bytes more of padding for every
function of the sandbox do not show in what a runtime counts).

With the two binaries run in turn, fifteen runs each: a `switch` in a loop takes 7.7% less time, an
empty loop 5.0%, a sum of integers 3.6%, arithmetic on doubles in a loop and nested loops 3.5%
each, a global variable in a loop 3.4%, the properties of one object 3.3%, a function that calls
itself and the arithmetic of a Float64Array 2.9% each, a method 2.2%, `new` of a class 2.1%, a
plain call 1.6% and a closure 1.5%. A step of the particles is where it was (2.4% more in that
run, 1.2% less in the next) with 3.9% fewer instructions: it retires them in the cycles it took
before, so its time is not the count of its instructions.

A value that was just computed and a constant were compared by three instructions: the value
pushed, the constant put in the result register, and the comparison that jumps (`p.x < 0`; a local
is compared with a constant by one). They are one instruction now, for the four orders both ways
and for strict equality and its negation, which has in an argument which of the ten it is
(`JumpIfResultComparesWithConstant`); `f() === 3`, which was two, is that one too. It is one
instruction with one arm for the ten, where the comparisons of a local have an arm each, twenty
of them.

Counted by the instructions the cases retire, a step of the particles is 6.2% fewer (its four
tests of a property against a number were sixteen instructions of the forty-five that a particle
takes, and are eight) and a tokenizer 1.2% fewer; the cases that call are 0.2 to 0.5% more. With
this instruction in the loop a call is three to four machine instructions longer by the price list
(479 for 476 with no argument) and `new` five to seven, whichever way it is written: as a special
stack slot of the twenty comparisons of a local, with no arm of its own, it cost the calls the
same, and with its comparison in a function that the arm calls it cost them the same and was six
longer itself. It was not timed against the round before: other work had the machine. The table of
the round's checks, one process for a case and taken under that work, has a step of the particles
at 2.15x for 2.21x and a closure at 2.25x for 2.21x, with both engines' times 3% higher than in the
table before it.

Going from one instruction to the next is nineteen machine instructions before the first of an
arm's own: the length of the code and its address read from the stack, two comparisons that the
window is in the code, the kind and the arguments read, the comparison that the kind has an arm,
and the seven of the jump through the table. Four ways to make that shorter were measured by the
price list, and none is kept. The place in the code kept as what is left of the code (a slice
whose start moves) made a turn of a loop 9 instructions longer and a call 38: it is two more words
that live across every arm, and the compiler keeps them on the stack. An arm for every kind, none
left to the arm of whatever remains, takes the comparison of the kind away, and with it the
compiler keeps the place in the code on the stack: a turn 10 longer, a call 20. One comparison
with where the windows end, for the two, is not what the compiler made of it in this function (it
kept the second, by a length it no longer knew to be the first's): 6 and 44 longer, with the arm
for every kind. And the general instructions called, not part of the loop: 28 longer, and 24 to
50. In a loop of one page each of the first three does what it says, and the comparisons go; in
this one what the compiler keeps in a register decides, and the loop as it stands is where the
rounds before left that.

An arithmetic or bitwise operator whose left operand is a local pushed the local, worked out the
right operand and then ran (`x * step` with `step` a variable of an enclosing function: three
instructions). Where the right operand does not assign to the local, the local is what it was
when the operator runs, and the operator reads it from its stack slot then
(`ApplyBinaryOperatorToIndexWithResult`): the push is not made, one instruction fewer. The
compiler asks the references of the local for a write inside the right operand, as it does for
the object of an assignment (area 4); a right operand that assigns to the local (`x + (x = 5)`,
`x * x++`) is compiled as before, and so is a literal, which the instructions with a constant
take. What runs in what order is unchanged: the right operand is worked out, then the two are
converted, left first.

It has an arm of its own. As one more kind in the arm of the operator on a local and a constant,
which it is all but the same as, it made every turn of a loop five machine instructions longer and
every call four to seven, and everything a percent or two slower by its instructions; with its own
arm a call is two to three longer and a turn of a loop what it was. Counted by the instructions
the cases retire, the call of a closure is 2.7% fewer and nothing else moves. With the two
binaries run in turn under the other work, the least of fifteen runs of that call is 1.8 to 2.4%
less.

#### The functions the loop calls

What the loop does not do itself it calls a function for: a property read or written by its cache, a
variable of an enclosing function or of the global scope, the arithmetic of doubles, the Number made
of a double. Each is a few dozen instructions of work, and the compiler put up to fifteen more around
them. A function keeps a frame (the frame pointer and the return address saved where it starts and
restored where it returns, and with them every register it keeps a value in across a call) wherever
it calls anything that comes back, however seldom, and wherever it can fail: an index out of range,
an `expect`, a handle that names nothing are each a call, and the compiler saves for them where the
function starts, not where they are. A handle that the message of a failure prints was written to
the stack there too, for the message, in every call. Making a Number of the heap was 41
instructions, ten of them for the one call that grows the heap's vector of Numbers.

So a function on the way of an instruction is written with nothing that comes back:

- It has no failure of its own. What is out of the ordinary (a handle that names nothing, a slot the
  storage has not, a record that is not there) is `None`, as a shape the cache does not know is:
  the instruction then goes the general way, and that says what is wrong where something is.
- What it calls, it calls last, and hands back what the callee returns as it is. A call made last is
  a jump; a function whose calls are all jumps keeps no frame. The rare work goes there whole: the
  Number of a double where the vector has to grow is made by the function that grows it, and a
  write to a storage that has descriptors by the function that asks them.
- A value is handed back as it was read. Taken apart into its kind and its bytes on one way out and
  put together where the ways out meet, it is five instructions, and the compiler, which joins the
  ways out first, no longer sees that a call beside them is last. Where a function has such a call
  and other ways out, it is two functions: one that only chooses and calls either (a variable of the
  global scope or of an enclosing function), and one with the work.
- The message of a handle that names nothing is made by one function for every accessor of a
  handle, which takes the handle in a register (`invalid_handle`): twelve accessors had it in line,
  in every function that reads through a handle.

By the price list, with nothing else changed: the two operators of `x * 0.999 + 0.25` 268 to 241,
`Math.floor`, `Math.sqrt` and `Math.sin` 28 to 31 less (their result made, `Math` read, the call
entered), a property read 145 to 137 and written 136 to 129, a variable of an enclosing function
read 79 to 74 and written 143 to 138, a global read 131 to 120, an element of a Float64Array read
298 to 283 and written 222 to 207, `new` 38 less, `Math.max` 515 to 498. Nothing in the list went
up. One frame is left where it was halved: the read of a property keeps two registers for the one
call it makes for an ordinary object that inherits from an Array or a function, since what it hands
back on its other ways out is put together from parts.

With this round's binary and the one before it run in turn, fifteen runs each (the calls of area 3
are in it too): a plain call reads 13.6% less time, a function that calls itself 12.5%, getters and
setters 10.3%, a method 9.6%, a closure 8.8%, a method inherited through three prototypes 8.4%, a
step of a generator 4.5%, `new` of a class 3.2%; arithmetic on doubles in a loop 5.2% less,
trigonometry 4.9%, the arithmetic of a Float64Array 4.4%, a variable of an enclosing function in a
loop 4.5%, the properties of one object 3.9%, a global variable 2.9%, a step of the particles 2.4%.
An empty loop, a sum of integers and nested loops are within half a percent of where they were and
a `switch` in a loop 0.7% under it: the loop of instructions, which has the return of a function in
it now, runs what it ran.

What an arm hands to such a function counts as well. The tables of the running code (its constants,
its caches, its binding sites) are three slices: six words that no register holds together. An arm
that calls a function with the tables wrote the six to the stack for it, and the compiler did that
where the arm starts, for every way through it. The arms that read a property by the cache each
have rare ways out to a function for what is no ordinary object (the method of a String or a
Number, a property of a function, the length of a TypedArray), and those took the tables, to find
the cache and the name in them: eight instructions of every property read wrote the tables for a
call that was not made. Those functions take the cache and the name now, fetched on the rare way.

A value was bound and unbound across the call that reads a property by its cache: handed back as a
value of the scope's lifetime, and made one of no lifetime where the arm puts it in a register of
the `Vm`, which the compiler does by taking it apart and putting it together, five instructions.
The function hands back what the heap keeps, as it keeps it.

And the name of a variable was a constant of the code while the place where it was found the last
time is an entry of another table: an arm that reads a variable at its site asked both tables, and
made a String of the Value the constant is (told apart by its kind, put together from its bytes).
A site keeps its name beside what it remembers (`NamedSite`): filled in from the instructions when
the tables are made, and kept current by the collector as the constants are. A build with
assertions checks at every use that it is still the name the instruction has.

By the price list: a property read is 124 instructions for 137 and an inherited one 167 for 180; a
variable of an enclosing function is read in 57 for 74 and written in 122 for 138; a global is read
in 102 for 120 and assigned in 332 for 367; a method call is 718 for 731, `Math.floor(i / 3)` 553
for 586, `Math.max(i, 7)` 430 for 462, `s.charCodeAt(i)` 488 for 506, a callback of `forEach` 756
for 774, and a step of a generator 1060 for 1067. Nothing went up, and the loop of instructions is
89 instructions shorter. With the two binaries run in turn, fifteen runs each: a variable of an
enclosing function in a loop reads 7.0% less time, the properties of one object read and written
6.4% (what the round before had cost them, and four more), calls of `Math` 5.3%, a closure 4.5%, a
step of the particles 4.1%, a global variable in a loop 3.9%, a method 3.8%, a callback 3.7%; a
plain call, a sum of integers and a `switch` in a loop are within 1.3% of where they were, and an
empty loop reads 6.4% less, by where its code lies now.

Tried on the way and not kept: the Number of a double handed back through an `Option` for "the
vector has to grow" (the test for it was joined into the way out of every Number, four instructions
for the five saved); a write by the cache that tried its straight way and left everything else to
the function it had been (a setter was 26 instructions longer and `new` 10, by the cache searched
twice: the function keeps its ways out and only its two calls became calls made last).

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

A global that is a property of the global object is read in 131 instructions more than a local, from
156 (QuickJS: 41), and with it every call of `Math.sqrt` and its like, which read `Math` first (25 of
about 600 each). Half of the read after the state of the global scope is found as it was had been
the slots of the global object taken as a whole (its shape for their number and their capacity, one of
twelve vectors by the capacity, a slice of that) to read one of them; now the one slot is read, as the
property cache reads a slot. That reader was part of the function of the cache while it was its only
user, by the compiler's choice, and a function of its own once there were two: a property that is
inherited was read in ten instructions more, a method of a prototype called in ten more. So the
reader and the three small functions under it (the slot of a storage, the storage by its index, its
range) are marked to be part of whoever asks, one at a time down to where the function of the cache was
the instructions it had been (compared by the instructions; the price list had shown each step). An
assignment to such a global is 380, of which a quarter is the name resolved before the right-hand
side is worked out, a quarter the slot written after the descriptors of the global object's storage
are asked, and a sixth the state of the global scope found as it was a second time: not changed.

The functions that read and write a variable where it was found the last time keep no frame now
(area 1, "The functions the loop calls"): the records on the way, and the state of the global
scope, are asked for what is there and are `None` where it is not, and the read is two functions,
one for the global scope and one for the enclosing functions, each called last. A variable of an
enclosing function is read in 74 instructions for 79 and written in 138 for 143, a global read in
120 for 131 and assigned in 367 for 380.

Each such place then asked two tables, the constants for its name and the binding sites for what it
remembers, and made a String of the name at every use. A site keeps its name beside what it
remembers now (area 1, the same section): a variable of an enclosing function is read in 57
instructions and written in 122, a global read in 102 and assigned in 332.

A variable of an enclosing function that is assigned to and read next (`total += x; return total`,
`n = next(); if (n) …`) was written where it was found, and then found again to be read. The
assignment leaves what it assigned as the result now (`PutBindingAtSiteAndGet`, one more kind in
the arm of the assignment), and the read is not made: the variable has what was assigned, once it
is set. Where the arm does not set it (an immutable name in sloppy code, which an assignment
leaves as it is; a name that is not where it was), the general way assigns and then reads. The
compiler joins the two only where the read is the instruction after the assignment and nothing
jumps in between: after a join of branches or at a turn of a loop the read is made as before.

Counted by the instructions the cases retire, the call of a closure that adds to a variable of its
maker and returns it is 7.4% fewer, and nothing else moves: the loops that assign to such a
variable or to a global are 0.3 to 0.4% more, the one comparison of the kind at each assignment.
With the two binaries run in turn under the other work, in two pairs of fifteen runs, that call is
4.3 and 6.2% less; the table of the round's checks, taken under that work too, has it at 2.18x for
2.34x.

What a site remembers was tried one step further, and that is not kept: beside its place, the
record the walk to the variable started from and the record it ended at, so that a walk from the
same record is not made again and the name is not looked at where it ends. (That holds from one
collection to the next, which forgets it for every site, and while no binding is removed from a
record, which moves another to its place.) A read was 51 instructions for 56 and an assignment 107
for 118, the call of a closure 2.5% fewer. But twelve bytes more for every site were 119 KB of the
8 MB a widget has: a widget's runtime has ten thousand sites, one for every reference to a name
of an enclosing function or of the global scope, and a long widget no longer fit. The table of
sites may not grow. That it is 160 KB as it is, with a site for every reference and not for every
name, is the next thing to look at there.

The name at a place was compared as the String it is: its kind first, then what that kind keeps,
and the seven bytes of a short name four, two and one at a time. That was a dozen instructions of
every read and every assignment of a variable where it was found the last time, and the function
of the standard library that compares two arrays was 2.9% of the time of a closure's call. A
record keeps the names of its bindings as eight bytes each now, which two names have alike
exactly where they are the same String (the kind, then the seven bytes of a short name or the
index of a name of the heap), made once where a binding is created. The name that is looked for
is made eight bytes the same way, in the register it arrives in, and the two are compared at
once; the search of a record of few bindings compares the same way. An entry is the twenty bytes
it was, and a widget's memory what it was.

A variable of an enclosing function is read in 51 instructions for 56 and assigned in 114 for 118.
Counted by the instructions the cases retire, the call of a closure is 1.2% fewer, a loop that
adds to a variable of an enclosing function 2.6% and a tokenizer 0.7%; nothing else moves. With
the two binaries run in turn under the other work that call is 0.5% less and that loop 1.1% less;
with the two engines run in turn the call of a closure is 2.01 to 2.02x, for 2.04x before.

The function that reads a variable of an enclosing function asked a second time whether its site
is one of the enclosing functions', which whoever calls it has asked already: four instructions
of every such read. It walks without asking now, and the read is 48 instructions for 51. Counted
by the instructions the cases retire, the call of a closure is 0.8% fewer and a loop that adds to
such a variable 0.9%. With the two binaries run in turn under the other work each is 1.8% less,
and with the two engines run in turn the call of a closure is 1.99 to 2.00x.

A place in the code kept its name as a String beside what it remembers, and the name was made
eight bytes at every read and every assignment there: five instructions of each. The place keeps
the eight bytes now. The tables make them once, and again after a collection for a name of the
heap, which is at another index then; what reads or assigns the variable compares them as they
are. A read from a record after a collection that moved its names takes what it took before one
(three million calls of a closure with names of twenty-eight letters, made after two hundred
thousand strings of garbage: 280 ms with the collection and without).

A variable of an enclosing function is read in 44 instructions for 50 and assigned in 110 for 115.
Counted by the instructions the cases retire, the call of a closure is 1.5% fewer and a loop that
adds to such a variable 2.7%. An assignment to a name of the global scope that may not be bound
(`g = v` of a `var` or of a property of the global object) is 343 for 330: the Reference it makes
is made of the String, which is made of the eight bytes there, and its two arms are longer by what
the compiler made of them, whichever way the name is fetched (from the key, 340 to 343; from the
constants, 345). With the two binaries run in turn under the other work the call of a closure is
3.1% less and a loop that adds to a variable of an enclosing function 2.8%; with the two engines
run in turn the call of a closure is 1.96x.

The operator and the assignment of `total += x` as one instruction, for such a variable, were
tried and are not kept. The call of a closure was 0.35% fewer of its instructions with the
operator found by its number, and no fewer with `+` tried first as the operator it is, each call
two to three machine instructions longer with the arm: what one dispatch saves, finding the
operator and one more arm cost.

A read of a global that is a property of the global object asked every time whether the global
scope was as it had been seen: the number of the state, two counters of the heap (the collections,
the keys of an intrinsic shape moved in place), the running realm, the shape of the global object,
the number of lexical declarations of the global scope, and then the record of the shape for the
capacity of the object's storage and the slots it uses: 55 of the 101 instructions of the read,
most of them a record read through another (QuickJS reads a global in 41). All of it but one
thing happens where the engine can say so, and seldom, so the global scope says when it changes
and the state is forgotten then: by a collection, which moves what the state names; by the three
functions that change the shape of an intrinsic object in place (a key removed from among the
others, private names put in front, a key added at the end, which moves none and counts only
when the values go to a storage of another capacity); by the two functions of the global record
that add a lexical declaration, which may be of a name that was a property. A realm that is made
is counted, and the running realm is compared with the one of the state only in an agent that
has made more than one. What nothing tells of is asked: whether the global object has the shape
it had, which any property added to an ordinary object changes. The capacity of the storage is
kept with the state, the storage itself is read from the object's record with its shape, and the
slot is read as the property cache reads one of a capacity it knows, without the count of the
slots in use: a place that is remembered is in use until a key is removed. No place is
remembered in a storage of more than 65,536 slots, which is read by a call, for which the
function kept a frame on every way through it.

Each of those tells was a comparison because a place read in a state it was not found in is a
wrong value and no failure, so each is tested by what goes wrong without it. Three scripts of
one global scope add 1,200 properties to the global object while its variables are read, remove
some, put an accessor in place of a value and back, collect, and then declare lexical names over
properties, where the first thing a later script does is call a function of an earlier one by a
lexical name, so that no property of the global object is found anew, which would see the state
changed, before the old places are read. With the tell of a removed key taken out, or of a
lexical declaration, the test fails at the name it finds at the place; with that of the capacity
it never ends (the counter of its loop is such a variable). A collection moves nothing that the
state names in a runtime of one realm, so no script tells of it: a test of the engine asks
whether the state is remembered after each of the four.

A global is read in 60 instructions for 101 and assigned in 303 for 343, and every call of
`Math.sqrt` and its like, which reads `Math` first, is 41 fewer (434 for 476; `Math.max(i, 7)`
373 for 415). Counted by the instructions the cases retire, a loop over a global is 11.5%
fewer, trigonometry 7.6% (with the sweep of area 9), calls of `Math` functions 7.1%, a frame of
easing 2.0%, a class that calls its parent's methods 1.4%; nothing is more. The reader of a slot
had the function of the property cache as its one caller, and was a function of its own again
with a second: a property was read in eight instructions more. The read of a global has a reader
of its own for the capacities that are not read by a call.

An assignment to a global that is a property of the global object asked the state of the global
scope three times: for the Reference that is made before the value is worked out, for what the
Reference is to when the assignment is made, and for the place. It went through the realm for
its global scope twice, and wrote the slot after reading the record of the object's shape for
the capacity and the slots in use. The state keeps the global scope now and the Reference is
made of it; the assignment asks the state once, for what the Reference is to and for the place
both, and writes the slot by the capacity the state keeps, as the property cache writes one of
a capacity it knows. That is 236 instructions for 303.

Where the value is worked out without the reference register (a literal, a local, the
arithmetic and the choices among such: what the compiler asks already before it saves the
register around the value) no Reference is made at all. An instruction of its own
(`CheckGlobalAtSite`) finds the name bound at its place and leaves the register empty, and the
assignment writes at the place or, when it has to go the long way, makes the Reference then. It
is the same assignment: a name that is bound in the global scope has one Reference whenever it
is made (the global scope, the name, strict or not), and what becomes of the assignment when the
name is gone or changed by then is what the long way makes of that Reference. The value may
still be worked out by code of the script, a `valueOf` of a local, which removes the name,
removes it and makes it again, puts an accessor in its place, forbids writing it, adds
properties to the global object or runs the same assignment again: the test does each, in
functions that are strict and not, with names that another script declares lexically, and with
the register holding something else when the assignment begins. A name that is not found bound
at its place is resolved as it was, its Reference in the register: unbound when the assignment
begins, it is a ReferenceError in strict code even where the code that works the value out
makes it (V8 assigns there, and Node's output differs from the expected by those two lines).

An assignment to a global is 163 instructions for 303 (QuickJS: 61). Counted by the instructions
the cases retire, a loop that adds to a global is 5.8% fewer (its `+=` reads the name first and
is written by the capacity), a plain call 0.3% and the call of a closure 0.2% fewer; nothing is
more. The write of the slot made part of the function that asks, where it is a call, was 162
and is not kept.

`undefined`, `NaN` and `Infinity` were read as any name of the global scope is: sixty
instructions each to find a property of the global object where it was found before, and
`x === undefined` was no comparison with a constant, nor a test and a jump in one. All three are
properties of the global object that are not writable and not configurable, and a Script cannot
declare any of them (it is a SyntaxError), so where the scope chain of the name is fixed all the
way out, which is where a global is read at its site, the name is the value: a constant of the
code. A parameter, a variable or a declaration of that name in a function, the code of an `eval`
and of `Function`, and an assignment to the name are as they were (an assignment does nothing, and
throws in strict code). `x === undefined` of a local is one instruction with its jump, and of a
variable of an enclosing function 111 instructions for 209 (QuickJS 99); `this === undefined` is
106 for 229 (108); a read of one of the three is a constant put in a register.

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

The six functions that ask what a number is were not among them, and each was the whole call for
one test of a double: 680 instructions for `isNaN(x)` and `isFinite(x)`, 815 for `Number.isNaN`,
`Number.isFinite` and `Number.isInteger` (QuickJS 190 to 245; found by pricing what ordinary code
does, area 6). They are marked now. `isNaN` and `isFinite` of a Number are answered in place, and
of anything else called, since the call converts its argument and may run a script for that. The
four of the Number constructor convert nothing, and are answered in place for any value: what is
no Number is none of them. Each of the four is one function for the call and for the instruction,
so that the two cannot come apart. `isNaN(x)` is 215 instructions and `isFinite(x)` 217 (QuickJS
188 and 198), `Number.isInteger(x)` 379 and the other two of a double 400 (235 to 245: what is
left is the read of `Number` and of the function off it). Nothing else is changed, by the
instructions that fifteen cases of every kind retire. The test has each of the six on values of
every kind, with no argument and with more than one, the conversions in their order and those
that throw, each function called through a variable, `call`, `apply`, another object and as a
callback, and a function replaced or shadowed; with any of these it fails: one of the two answers
given for the other, a value that is no Number answered by `isNaN` unconverted, or taken for one
by the four, a safe integer taken for any integer, one function marked as another, or the last
argument asked in place of the first.

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

A fifth of what was left was not the object but its descriptors. `length`, `callee` and the iterator
of an arguments object are not plain data properties, so every object had a map of three descriptors
made for it, put into the table of its storage's capacity under the storage, and taken out by the
collector, which first gathered and sorted the keys of the storages that were gone. The three are the
same for every arguments object of a realm, so the table of descriptors (`DescriptorTable`, area 4)
keeps them once:

- `CreateUnmappedArgumentsObject` asks the table to share what it would have made. A storage that
  shares is marked by a bit and has no entry of its own. Where the table already keeps descriptors
  and they are not these (another realm's `%ThrowTypeError%`), the object gets its own as before.
- Anything that is to change the descriptors of a storage gets them through the table's entry for
  it, and that gives a storage that shares a copy of its own first: one arguments object with its
  `length` made read-only, its `callee` asked to be redefined, its iterator deleted, or frozen, has
  its own descriptors from then on, and the others what they had. A write of `length` by a cache,
  and every reading, sees the kept ones through the same two functions as before.
- The collector moves the bits as it moves the storages, has the handles in the kept descriptors
  seen to once, and lets go of them when no storage shares any more. The entries of storages that
  are gone are dropped where they are, without a list of their keys.

`call/arguments-object` went from 2.2x to 1.7x (26.7 to 20.3 ms with the two binaries run in turn).

How the table holds this mattered more than what it holds. The first form added two fields to it (the
kept map and a second vector of bits) and gained as much, but there is a table for each of the eleven
capacities in the heap, and what is laid out after them is read by the loop of instructions: with
every table larger, `Vm::run` came out six instructions longer and other in 1,500 places, and four
cases of the panel were 2-4% slower (a plain call, `new` of a class, a callback), six with the lookup
kept out of line. The form that is in has the two kinds of bits side by side in one boxed slice and
the kept descriptors behind a pointer, which makes the table the 80 bytes it was: `Vm::run` is the
same 12,972 instructions (180 of them with other low bits of an address). A type that lies in the
heap in front of what the loop reads is not to grow.

One case stayed 3% slower in every form, `new` of a class, and it was not the layout. Whatever is to
write to a storage through its values and descriptors asks the table for the place of its
descriptors, and the table answered with an entry of its map: the hash of the storage, a search, and
room reserved for one more, for a storage that nearly never has any; the check for sharing went in
front of that. The place of a storage with no descriptors is now the test of its bit, and the map is
searched when descriptors are given to it. `call/new-class-instance` is where it was (21.5 ms in both
binaries), and the arguments case gained two points more by it. The panel read 0.8% less, with the
one case it showed worse (a method found on a prototype, +2.2%) at +0.9% over nine runs each, inside
what the runs spread.

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

A call and its return by line again: 57% of `call/plain-function` is entering and leaving (the
execution context made and pushed 8%, the frame and the stacks 17%, what the function is and whether
it may be run in place 5%, the return 22%), and two things in that were not work.

Leaving cut the Reference stack and the iterator stack back to where the caller had them, and released
the collections that the function had scoped. Nearly every function leaves all three as it found them,
and cutting a vector of what has a destructor by nothing still calls for the dropping of the nothing
that is cut off; the release was a call that compared two lengths. Each is done now only where there
is something to cut: a twentieth of a plain call.

The Return that ends a function is the last instruction of its code, and the loop reads an instruction
together with the four bytes after it, in one look at how much code is left: with fewer than four left
the instruction was read the other, slower way, at every call of nearly every function. The
instructions of an Executable have four bytes after them now that are no instruction (`Bytecode`). The
loop reads from the padded bytes; where the code ends, what a listing shows and what anything else
reads are the instructions without the padding, which is all that the type gives to whoever does not
ask for the padding by name.

With the two binaries run in turn: a plain call 8.46 to 7.97 ms (2.18x to 2.06x in the full runs), a
method that three prototypes up has 8% less (2.30x to 2.16x), `new` of a class 6% (2.04x to 1.89x) and
of a derived one 5% (2.05x to 1.97x), a method 5%, a closure 3-4%, a bound function 6%, the callbacks
of `map`, `filter` and `find` 5%. The recursion of `fib`, the arguments object and a generator are
where they were. An empty loop reads 4.6% slower (3.30 to 3.46 ms; 0.98x to 1.02x) and a sum of
integers 1.8%, by where the loop of instructions landed: neither runs anything that changed, and the
empty loop reads 3.30 or 3.50 from one run to the next of the binary before. Tried against that: the
instructions for the slow way of reading taken from the padded bytes instead of from the tables (the
calls gained half as much, the empty loop the same), and the padded bytes and the instructions as two
slices in the loop (a `switch` 9% slower).

The object that `new` of a function of the script starts from was made as the general way makes it: an
object with the Realm's %Object.prototype%, then given the function's "prototype". That is two shapes
looked up in a table by their prototypes for every object made, and the prototype's own shape seen to
each time, where the first object made with a prototype sees to it once. It is made with the shape of
the objects that have that prototype at once, which is the shape the other way arrived at. `new` of a
class went from 22.9 to 21.7 ms with the two binaries run in turn (`call/new-class-instance`, 1.86x to
1.84x in runs where QuickJS's own time moved as much), and with `super(…)` 3% less.

`new` of a function that sets one field was still 2,138 instructions where QuickJS has 1,074, and
the profile of it had no one thing over a twentieth: but half a dozen of them were the same three
questions, asked the long way at every object made.

What the function's "prototype" is: by Get, which went through the function's own way of reading a
property and read the slots of its properties as a whole for the one, 270 instructions with the
descriptor of what it found. It is read as a property is read by its cache in the loop of
instructions, one look at the storage; Get answers where that does not, and the cache sees the
shape then. The parent of a derived class reads the new target's the same way.

What shape an object of that prototype has: by a table of such shapes, keyed by the prototype,
whose keys are compared kind by kind. The shape found the last time is kept with its prototype
(an ordinary object, compared as the handle it is) and is the answer while no collection has run.

What shape an object comes to when a key is added to it: by the table of the shape's transitions,
under a hash of the key, for every field of every object. The shapes arrived at the last time are
kept, eight of them by the place of the shape they start from, with the key, for as long as no
collection has run: a constructor adds the same keys to the same shapes every time. And adding a
plain property asked the table of descriptors for the place of the storage's own, to put nothing
there.

Each of the two things kept is of the collection it was found in, and the tests of that took three
tries. In a runtime nothing that is kept is ever found where another thing was: a prototype that has
had an object made of it does not die (the table of shapes by prototype holds it), a shape that a
property cache has seen does not either, and the first object made after a collection is rarely of
the prototype whose place moved. So a test of the engine makes things die in front of the ones kept
(objects in front of the prototypes of three makers; shapes that no cache holds, made by
`JSON.parse`, in front of two chains of shapes with one of another kind between), collects, and
makes objects of the others; how many must die for one to land in the place of another is the
heap's business, so it tries every number up to a dozen, each in an agent of its own. Without the
count of collections on the first, an object of one maker has another's prototype; without it on
the second, an object has the keys of another kind. The other three things asked of what is kept
(the prototype, the shape, the key) are caught by the script that is checked against Node.

`new` of a function is 1,840 instructions for 2,138 and of a class 1,812 for 2,110 (QuickJS: 1,074
and 1,130). Counted by the instructions the cases retire, `call/new-class-instance` is 8.4% fewer,
`call/class-inheritance-super` 5.1%, `json/parse` 3.3% and `prop/object-spread-assign` 3.2% (their
objects come to their shapes by the same keys), a tree built and walked 0.8%; the calls are the
same. What is left of adding the field is making its storage, which is the general way of growing
one from nothing (some 200 instructions for two slots), and the cache told of the new shape.
With the two binaries run in turn a derived class is 3 to 5% less (whichever is run first), and
with the two engines run in turn `new` of a class is 1.50x for 1.80x and a derived class 1.62x for
1.76x.

`typeof` of a value was an instruction of the general way, every time: the loop left for it, and came
back with the String. It is done in the loop where the value is in the result register and the
reference register is empty; `typeof name` of a name that may not be bound comes with its Reference
and goes as before. A loop of `typeof v === "number"` tests went from 9.7 to 8.0 ms
(`loop/typeof-and-truthy`, 1.52x to 1.28x), and the plain loops and the calls are where they were:
this arm, unlike the one for `yield` (the defects), moved nothing.

`typeof v === "number"` was still two instructions and a String: Typeof put the name of the type in
the result register, and the comparison with the constant read the two Strings to find them the
same. The profile of every case of the interpreter showed the two at 30% of
`loop/typeof-and-truthy`: the operator is one jump by the kind of the value, which the processor
does not guess where the kinds change every turn. Where the constant names a type, the comparison
that jumps is now of the value's type itself: the same instruction
(`JumpIfResultComparesWithConstant`) with two more of its comparisons, the type and not the type,
whose second argument is the type in place of a constant, and no Typeof before it. The type of a
primitive is told by the kind of the value; "object" and "function" by the operator, which asks an
object whether it is called. `==` and `!=` of `typeof` and such a constant are the same test, two
Strings being loosely equal where they are strictly equal. What Typeof takes as a Reference (a name
that may not be bound, a property) the instruction takes the same way: it leaves it to the general
way, which is Typeof and the comparison as they were. A test whose value is used
(`var isNumber = typeof v === "number"`), the name on the left, and a `switch` on `typeof` are as
before.

`loop/typeof-and-truthy` retires 16.8% fewer instructions and is 0.83x for 1.05x with the two
engines run in turn. By the price list nothing else moved: a call with no argument is 478.9
instructions for 479.3, the plain loop 212.9 for 214.0. The test has every type by every operator
on seventy values of every kind, the tests of `if`, `?:`, `&&`, `||` and the three loops, Strings
that name no type, a value that is itself the name of a type compared with no `typeof`, what goes
as a Reference with something computed just before it, and the value of a statement that ends in
the test, which is what the instruction leaves in the result register. With any of these it
fails: a Reference not left to the general way, a kind of value left out of a type, the jump the
other way in the loop or in the general way, another type's name there, the register not emptied,
a constant taken for the name of a type that only starts like one, or the comparison made of a
value that no Typeof was before. (Three of them made a loop of the first script endless, which
fails nothing in a time anyone waits: its loops now end by a count too.)

The call of a closure is the second worst case now (`call/closure`, 2.7x), and nothing single is
left in it to take out. Its body is eight instructions. Entering the function is a fifth of the time
in some twenty pieces of 1-2% each (the execution context pushed, the stacks exchanged, the
arguments put where the parameters are, the code rooted, its tables fetched), leaving it a tenth,
and the three reads and one write of the variables it closes over 3 ns each (a sixth together): the
running environment asked of the execution context, the record found by its kind, the binding by its
place and checked by its name. Less than that is another way of making a call, not a change to this
one.

A call that the loop of instructions runs itself asked two things first: how many calls are in
progress, and whether the native stack is used up. The second has one answer for as long as the loop
runs: such a call uses none of the native stack, which is where it was when the loop was entered, and
the loop asks then. Asked again at every call it was 15 instructions of the 570 to 960 a call is (the
price list, "What an operation costs"), for a plain call, a method, a closure, a getter or a setter,
`new` and a generator stepped in place alike. Without it the cases that call read 0.7-1.8% less time,
the two binaries in turn. A call that goes through a built-in still asks both, and too deep is a
RangeError at the same depth for either kind. Looked at in the same place and not kept: the global
this value that a function which is not strict gets for a this of undefined, found through the realm
and its global environment at every such call. Kept from the last call it saved nothing that the
price list shows.

What was left of a call was spread as before: a quarter of a plain call's time enters the function
(two fifths of that is its execution context, made of what is read from the function's record and
pushed; the rest fills the frame), a seventh leaves it, a twentieth asks whether it can be run in
place at all. Two things in it were not work, which the instructions of the entry and of the return
read one by one showed.

A function that is not strict has the global object for a this value of undefined or null, and
every such call found it: from the function's realm to the realm's Global Environment Record to its
\[\[GlobalThisValue]], 25 instructions for a plain call, a closure or a callback, nearly none of
which ever says `this`. The execution context of such a call now says that its this value is the
global one, not found yet; the first `this` of the call finds it (ResolveThisBinding, which the
instructions of the loop that read `this` from the context leave for, as they do for a call that has
a Function Environment Record) and keeps it there for the next. A strict function, a method, a
this value given by `call`, `apply` or `bind`, and a primitive made an object are as they were.
(A value kept from the last call had saved none of it, the round before: it has to be checked.)

Leaving a function was a function of its own: every return saved fourteen registers and restored
them, and handed the calling code back through memory, around some sixty instructions of work. It
is part of the loop where a function returns, which is where every call ends; a function that
throws, or that a termination ends, is left by a call as before. What only some returns do (the
object of `new`, the result of a derived constructor, the generator of a step, a frame's value
stack that grew large, what the function left on the Reference stack or the iterator stack, the
frames of a deep recursion) is in functions of its own, so that what every return does is short.

By the price list a plain call went from 567 to 493 instructions (27 for the this value, 44 for
the return, 3 in the entry: the function's code rooted without a borrow of the scoped values, which
nothing else holds then, and the handles it reads through, area 1), a method from 841 to 784, a
getter from 830 to 757, a setter from 815 to 769, a callback of `forEach` from 806 to 773.

A method call made a Reference for its this value. `o.m(x)` read the function by the cache of its
place, made the Reference of `o.m` (its base, its name, and whether the code is strict, which the
execution context is asked), pushed that to the stack of References before the arguments and popped
it after them, and the call took the base out of it: 28 bytes moved three times and a question to
the execution context, to hand one value from before the arguments to after them. The value is
kept on the value stack now, under the function. One instruction pushes the object and then the
function it reads from it (the object in the result register, or a local), and the call takes the
arguments, the function and the this value off the stack together
(`LoadPropertyWithCachePushThis`, `LoadIndexPropertyWithCachePushThis`, `EvaluateCallOnThis`). That
is for a property by a name the code states, in a call that is not optional and spreads no
argument; `this.m(x)` reads the this value by its own instruction and goes on the same way. A
computed name, `super`, a private name, an optional call, arguments that are spread and a tagged
template make their Reference as before, and so does the code where these instructions do not find
the function in the cache: the general way reads the property, takes the base of its Reference and
pushes the two.

By the price list a method call is 731 instructions for 784 and one inherited through a prototype
774 for 827; `Math.floor(i / 3)` 586 for 624 and `Math.max(i, 7)` 462 for 498 (a built-in that
takes no this value was run without looking for one, and now no Reference is made for it either);
`s.charCodeAt(i)` 506 for 553. A plain call is within one instruction of where it was.

The loop of instructions has no arm more for this. The two instructions that push are part of the
arms that read a property by the cache with its Reference kept, and the call is part of the arm of
the other calls. With an arm each, the loop was 440 instructions longer (the search of the cache
is read into every arm that asks it), and every instruction of every script five instructions
slower: a turn of an empty loop 253 for 228, a plain call 537 for 493. And the entry of a function
decides where its this value comes from in one place, the plain call first: asked a second time
further down, for what to cut off the caller's stack, a call that is no method call was twelve
instructions longer.

With the two binaries run in turn, fifteen runs each: a method call reads 5.4% less time, a method
inherited through three prototypes 6.3%, calls of `Math` 6.9%, `Math.floor`, `round`, `max` and `min`
in a loop 6.5%, trigonometry 4.7%; a plain call, a closure, `new`, getters and setters, an empty
loop and a sum of integers are within 1.5% of where they were. What it cost elsewhere, by where the
compiler put things: a step of a generator is nine instructions longer (1067 for 1058) and reads
2.4% more time, `new` three instructions, and the properties of one object read and written in a
loop, whose instructions are what they were, read 2.2% more.

What is left of a call is mostly entering it: its execution context (some sixty instructions, all
of it copies), the frame filled and the stacks exchanged (eighty), the code rooted and its tables
taken (thirty), and the loop left and entered again twice, for the call and for the return, with
the tables of the code taken to hand each time (fifty); leaving it is ninety. Halving that is
another frame: one value stack with a base for each call in place of a stack exchanged at every
call, and an execution context that is not made for a call the loop runs itself until something
asks for it.

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

The shape's record is no longer asked where the property is the object's own (it was 13 of the 85
instructions of such a read, and a load the rest waited for). An entry of a cache that places a
property among the values of the object keeps the capacity of that storage beside the offset, and the
read and the write go from the entry to the arena of that capacity. That holds because the shape
decides the capacity and the record of a shape does not change, with one exception, for which nothing
is kept:

- The shape of an intrinsic object (an object used as a prototype, a class) is that object's alone
  and is changed where it is as the object gains and loses properties: the three functions that write
  the length and the capacity of a shape's record are for such shapes only. A shape that is shared
  stays shared, since every shape keeps its parent alive, and a cache keeps the shapes it names.
- A property found in a prototype is read by the prototype's own shape, as before: that shape may
  change under an entry that stays right.
- The invalidations change entries where they are (an offset moved down, a property now found
  nearer), never into an entry of this kind, and the capacity is believed only of an entry whose
  offset still says that it is one. An entry gets its shape through three functions, each of which
  writes the capacity with it.

Debug builds hold every use against the shape's record (the capacity, that the shape is not an
intrinsic object's, the offset under its length), and the engine's tests, the semantics scripts and
test262 run so. With the exception for intrinsic shapes taken out, in a release build, the embedded
test of this reads wrong values from a prototype and a class at once.

The length that the shape's record also gave is not needed: a slot past what a storage is used to
holds no value, which reads as none and leaves the general way, as an accessor does. And whether the
storages of a capacity are flat or each a vector of its own (those of over 65,536 slots) is asked at
run time only for the capacities where it can be either.

An own property is about 60 instructions now. With the two binaries run in turn:
`prop/get-set-one-shape` 13.37 to 12.46 ms (2.42x of QuickJS to 2.25x), a step of the particles 4.18
to 3.96 ms (2.75x to 2.6x), `call/method` 13.5 to 12.9 ms (2.14x to 2.04x), `prop/four-shapes` 5%
less, `prop/getter-setter` 4%, `array/index-read-write` 2.5%, `app/easing-animation-frame` 3%. The
panel of 34 cases reads 0.8% less over all, six of them better by over 2% and none worse
(`typed/float64-math` read 2.5% slower in it and the same on nine runs each). The semantics script of
this round is the first the reference build fails: it reads a property deleted from a prototype
through the objects that inherit it, a defect of the cache invalidation that an earlier round
repaired; the script is held against Node.

An accessor was a call made the native way. Where the cache of the place knows the shape and the property
is an accessor, of the object or inherited, whose getter is an ECMAScript function, the read is a call of
the getter that the `Vm` runs itself (no arguments, the object as the this value, what it returns as the
result); and the assignment is such a call of the setter, whose result the frame drops. A class with a
getter and a setter went from 4.7x to 2.9x. Reads that keep a Reference for a call that follows
(`o.getter()`) take the general way still.

Finding the function for that call was a fifth of such a loop: the cache gives the object that has the
property and its place, and the getter or the setter was then read from that object's descriptors (its
storage found again, and two tables looked up), at every read and every assignment. The loop remembers
the last getter and the last setter it found, each with the object and the place (`AccessorSeen`), and
they are the functions of that place until the heap counts a change of descriptors: a property that an
object had defined again, or the properties of an object moved (one removed, private names put in
front), which puts another property at a place. The count is the one that the `length` of a
TypedArray is remembered by (area 6), and a collection ends what was seen, since it moves the object
and the function. `prop/getter-setter` went from 15.9 to 14.1 ms with the two binaries run in turn
(2.37x to 2.08x). An accessor that is no function of the script, and one found with other descriptors
counted since, go the way they went.

`v instanceof F` looked F[Symbol.hasInstance] up key by key, called the method every function inherits as
a built-in, and that looked F.prototype up key by key. Both lookups go through the property cache of
their name, and the inherited method of the running realm is done without the call: 4.4x to 4.0x. What is
left of that case is the call of `Array.isArray`. Functions, Arrays and other objects with
a backing object do not take the short way for their named properties yet.

That call was not the call: `Array.isArray` is a property of a function, and the read of a property by
its cache knew ordinary objects, Arrays, Strings and Numbers. A property of a function took the general
way of an instruction at every read (the instruction left the loop, a Reference was made, and \[\[Get]]
went through the function to its ordinary object and looked the cache of the name up in a table on the
way): a fifth of that case, and of every `Object.keys(…)`, `String.fromCharCode(…)`, `Date.now()`,
`K.prototype` and `f.call(…)`. A function that has properties has them in an ordinary object, its
"length" and "name" among them once it has one (a constructor with static methods, a function with a
"prototype", a class), and a property of the function is what a read of the name from that object
finds by the cache. A function with no such object has its "length" and its "name", which still go the
general way, and what it inherits, found by the cache under the shape of the functions that have
nothing of their own and read from the object that has it (`f.call`, `f.apply`, `f.bind`).

With the two binaries run in turn: `prop/has-own-and-in`, which calls
`Object.prototype.hasOwnProperty.call(o, name)`, went from 20.6 to 16.1 ms (2.13x to 1.61x),
`prop/instanceof-and-array-check` from 13.5 to 10.8 ms (2.33x to 1.86x), characters made by
`String.fromCharCode` and joined from 1.85x to 1.62x, `math/random-and-date` from 2.08x to 1.88x,
`call/spread-and-apply` 6% less. The calls and the loops are where they were. Appending one
character a hundred thousand times reads 4% slower by the medians (`string/append-100k-char`, 1.94x
to 1.98x), with nothing of what it runs changed.

The object that has an inherited property of such a function is read the general way, not by the
one function that reads a slot of an ordinary object. That function is part of the read of every
property by its cache for as long as nothing else calls it; called from here as well, it was a call
there too, and a loop that reads the properties of one object was 5% slower and a method call 3%
(the same source builds the same binary: it was not chance). Made part of both by force, half of that
came back; left to its one caller, all of it.

A method of a Map, a Set, a RegExp or a TypedArray was the next read of that kind: `map.get`,
`set.has`, `regexp.test`, `bytes.set` each took the general way at every call (the instruction left
the loop, a Reference was made, \[\[Get]] asked for the shape of the object by its prototype in a
table searched by hash, and found the method by the cache after all): a quarter to a third of the Map
and Set cases, a tenth of a RegExp tested in a loop. None of these objects has a property by a name
of its own kind, but for the "lastIndex" of a RegExp and the names of a TypedArray that are the
strings of Numbers ("NaN" and "Infinity" among them), which are its elements and are never looked for
in a prototype; those are left to the general way, the second by the first byte of the name.
\[\[Get]] of one is OrdinaryGet: of the ordinary object that has its properties once it has any (a
subclass, an object given a property or another prototype), and until then of what it inherits, which
the cache places under the shape of the objects that have its prototype and nothing else. A Date, a
Promise, an ArrayBuffer, a DataView, the iterators of Arrays, Maps and Sets, a generator, a WeakMap,
a WeakSet and a WeakRef are read the same way.

The shape of such an object is the one the heap keeps for the prototype of its kind in the running
Realm. The table has one shape for a prototype, put there once and taken out by a collection only, so
the shape last found for a kind in a Realm is remembered (`BareShapeSeen`) until the next collection:
a quarter of what the change gained on the Map cases.

With the two binaries run in turn: `set/add-has` went from 3.23 to 2.26 ms (1.50x to 1.05x),
`map/string-keys` from 1.39 to 1.06 ms (1.74x to 1.34x), `map/number-keys` from 4.34 to 3.33 ms
(1.67x to 1.27x), `typed/set-subarray-copy` from 0.36 to 0.31 ms (2.00x to 1.70x) and
`regexp/test-simple` from 6.71 to 5.98 ms (1.83x to 1.56x). The loops, the calls and the properties
of ordinary objects are where they were.

Where the new code and what it remembers are put decided that last sentence, twice. As part of the
function that reads the properties of functions it made that one 980 bytes longer, the read of a
property of an ordinary object, which follows it in the binary, moved by as much, and a loop of such
reads was 3% slower: it is a function of its own. And what it remembers was a field of the agent
first: the code that makes an engine grew by 12 bytes, the loop of instructions, the same
instructions otherwise, started 4 bytes later, and `loop/switch` was 9% slower by the best runs and
15% by the medians. Kept in the heap, the loop of instructions is the same instructions at the same
places, 275 of its 13,274 reading the agent 24 bytes further on (and 89 more with other low bits of
an address, which move with the code). (This said "byte for byte" at
first, by the hash in the function's symbol, which does not tell: it is of the function's name and
type, not of its code. The code of two builds is compared as such since.) Neither
was a property of the change: both are where this benchmark's binary happens to put things, which
another build of the engine does not share, and the cases that move so by nothing are to be read
with that. One function cannot be given an alignment in this toolchain (1.96.1); all of them can,
which was tried later and was not what moves them ("Where a function lands", above).

Every write of a property by its cache asked whether a descriptor forbids it. The storages of one
capacity have their descriptors in a map by the storage, and the map was searched by the storage's
hash at each write, to find, nearly always, that the storage has none: a tenth of a loop writing the
properties of plain objects, and a fifth of an assignment to a global variable, whose object has
descriptors, so that the search went on into them. The map is inside a type of its own now
(`DescriptorTable`), which keeps beside it a bit for every storage that has descriptors, and the slot
it last found writable in one that has. A write looks at the bit, and so does every reading of a
storage with its descriptors (`Object.keys`, a spread, `JSON.stringify`, `hasOwnProperty`).

The bits cannot be wrong by an oversight. The map is private to that type: a storage gets its first
descriptors only through the table's own `insert` or the vacant entry it hands out, both of which set
the bit, and what the collector does to the map goes through one function that sets the bits anew
after it. A bit may stay set for a storage whose descriptors were taken away, which costs the search
and nothing else. The slot last found writable is forgotten by every method that gives the map out
to be changed, and none changes it otherwise. Unit tests hold the table's answers against a search
of its own map after each kind of change. (The table also keeps once the descriptors that many
storages have alike, with a second bit for each storage that has them: area 3, the arguments
object.)

With the two binaries run in turn: an assignment to a global variable in a loop went from 4.79 to
4.49 ms (`loop/global-variable`, 2.19x to 2.03x), `prop/get-set-one-shape` from 13.9 to 13.5 ms
(2.53x to 2.40x), `prop/object-spread-assign` and `json/stringify` 3.5% less,
`app/easing-animation-frame` 5% less, a step of the particles 1-3% less. `loop/switch` reads 1%
slower by the best runs and 3% by the medians, with the loop of instructions the same instructions
(356 of its 13,274 reading the agent at another offset: the table is larger than the map it took the
place of, and what the agent has after it moved) and 2368 bytes further on in the binary.

What is left of a write to a global variable is spread thin: the state of the global scope checked
by five comparisons over a dozen reads, the object and its shape read, the storage found by its
capacity, the slot. Of a read, the same and a call for the values of the object (8% of that case and
of a loop calling `Math.max`), which stays a call: the one function that reads a slot of an ordinary
object keeps its one caller (above).

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

One by one, each of those assignments still looked for the key in the target, then in the object the
target inherits from, and took the target from one shape to the next: the descriptors made of
properties that are not there were a tenth of the case, and the assignments were most of it. For a
target that has no properties yet, of the prototype the source has, `Object.assign` now makes the same
copy as the spread, of the shape and the values, where it is the same thing. It is not always:
`Object.assign` assigns where the spread defines, so a property of the same key on an object the
target inherits from has its say (a setter is called and the target gets no property; one that is not
writable throws). So the keys of the source are looked for on the objects the target inherits from,
which are few and have few keys (`Object.prototype` has seven here), and the copy is made only where
none has any of them and each is an ordinary object; otherwise it is the assignments one by one as
before. The test has a setter and a property that is not writable on `Object.prototype`, a setter on
a prototype under a property of the source's own, a class field over a setter, a Proxy and an Array
for the prototype, targets and sources of every other kind, and a setter that `Object.prototype` gets
after copies were made; with the look at the inherited keys taken out it fails.
`prop/object-spread-assign` retires half the instructions it did and is 0.90x with the two engines
run in turn, from 1.90x: faster than QuickJS, which goes property by property.

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

What that left on the Reference way for being as many instructions was not as cheap: `p.x += 1`
and `p.x += i`, whose value leaves the reference register alone, read the property with its
Reference kept and assigned through it, and each was dearer than the same assignment written out
(`p.x = p.x + 1`), which reads the property and assigns it by the slot of `p`: 314 instructions
for 283, and 362 for 308 with a local for the value. They are compiled as the written form now,
where `p` stays in its slot through the value, which for such a value it always does. A step of
the particles, which has one of these for each particle (`p.vy += 0.05`), retires 1.1% fewer
instructions; nothing else moved. The test has every compound operator with a constant and with a
local, a getter and a setter in their order, what cannot be written, inherited and proxied
properties, a base that is no object, and the object replaced by the value, which fails where the
object is taken from the slot regardless.

The read and the write of a property by its cache were each a function with a frame, for the
handles they read through and for the descriptors a write asks of a storage that has any. Neither
has a failure of its own now, and the write asks the descriptors by a function called last (area
1, "The functions the loop calls"): a read is 137 instructions for 145, a write 129 for 136, and
what is built of them less by as much (`new` of a function that sets one field 38, with its call).
The read is 124 since: its arm no longer writes the tables of the code to the stack for the
functions it seldom calls, and takes the value as the function hands it back (the same section).

`o.x`, where `o` is a constant, a parameter or a function of the surroundings, is two instructions:
the name, then the property of what it has. Where the function keeps the value of such a name in a
stack slot for the call (area 2: a name that nothing assigns to, read in a loop), a read that
follows another read of the same name is one instruction, the one that reads a property of a local
with that slot for its local (`GetKeptPropertyWithCache`: the same arguments, and the site of the
name in the two bytes that the other leaves unused). The compiler emits it only where the code has
read the name before with nothing jumping in since. The slot is filled then, by the short way of
the read before, or the name has no value to keep (`undefined`, which has no property that the
cache places: the instruction goes the general way, which says what a property of that is). The
first read of such a run of code stays the two instructions it was.

The site has the name, for which the instruction has no room: the compiler says for every site
where its name is among the constants (the tables found that out by reading the instructions
before), and the compiled form of a script carries the list to every engine that runs it. The
tables keep the names and not the list. Kept too, it was one more block of memory for every
function of a script in every engine: 109 KB for the functions of the widget sandbox, which the
widget-sandbox suite noticed in a test that takes a widget to within 82 KB of its 8 MB.

By the price list a second property of the same object is 94 instructions for the 122 of the first
(QuickJS: 53), and nothing else is longer; a call is one shorter. With the two binaries run in
turn, fifteen runs each: the properties of one object read and written, whose line reads three
after a first, take 6.4% less time, the arithmetic of a Float64Array 2.2%, a plain call 1.9%. A
`switch` in a loop reads 3.0% more, a step of the particles 2.1% and a variable of an enclosing
function in a loop 2.0%, each with the instructions it had: where the loop's code lies.

The first read is left alone on purpose. Five ways of having the instruction fill its slot itself
were measured first, each by the price list, with the read at 93 to 98 in all of them:

- The name read and the property read again in a second turn of a loop inside the arm: the turn of
  a plain loop 234 instructions for 223, a call 7 to 12 longer.
- The slot filled by a function of its own that the arm hands the site, the name and the slot, and
  the instruction then read again: the turn of a plain loop 229. The arm kept the slot across its
  read of the property, the compiler gave it a register of the loop for that, and the loop then
  wrote the arguments of every instruction of a script to the stack and read them back, two
  instructions at each.
- The same with the slot read from the instruction again where it is wanted: the loop as it was,
  and every read of a property of a local one longer, by the place of the instruction that was
  kept beside the place of the next one for the second reading.
- The whole first read in a function of its own that is handed the code and the place of the
  instruction, whether it hands back the value or only says that it did: every call 5 to 8 longer,
  and that first read still 161 instructions more than the two it stands for.
- Nothing in the arm, and the slot filled on the general way of the instruction: the loop and the
  calls as they were, and the first read of a call 327 instructions more than the two instructions
  it stands for. A function that reads once in a short loop paid that at every call, and only a
  thirteenth read in the call made it even.

A function that has no loop keeps no names, and reads `o.x` by its two instructions as before.

What a read found was written to the result register in four pieces. The arm had the value handed
back, looked whether there was one, and wrote it: its kind as one byte and its seven bytes as four,
two and one, after putting the value together from the two parts the compiler kept it as on the
way; ten machine instructions for one store. And what is written in four pieces is not there whole
for the instruction that loads the register next until all four are. The reads by a cache (of a
value in the register, of a local or a kept name, of `this`) now write what the function that
reads the storage hands back straight into the register, as the eight bytes it comes as, and ask
the register whether it holds a value. Nothing found leaves the instruction to the general way,
which reads its base again or finds it put back.

By the price list a property read is 101 instructions for 111 and an inherited one 144 for 154, a
second property of the same object 77 for 87 (QuickJS: 53), a method that reads `this.n` 8 less,
and nothing is longer. Counted by the instructions the cases retire, the properties of one object
read and written are 3.8% fewer, a step of the particles 3.0% and the call of a method 1.2%. In
time, with the two binaries run in turn on a quiet machine, the cases that read no property are
where they were (the calls 1.6% less to 1.8% more, a loop that does nothing else 2.4% less); the
cases that read properties were not timed that way, other work had the machine from there on. The
table of the round's checks, taken under that work, has a step of the particles at 2.21x for 2.33x.

`--p.life` and `o.count++` were a read that keeps its Reference and then three to five
instructions: the value made a Number, the step, the assignment by the cache, and, where the value
of the expression is used, a copy pushed before or after the step and taken back after the
assignment. They are one instruction after the read (`StepValueWithCache`), which has in an
argument which step it is and which value it leaves. Its arm steps an integer that stays one and
assigns it where the cache places a writable property of the object's own; anything else (a
double, a text, a BigInt, an object that is converted, an accessor, an inherited property, a
Proxy) goes the general way, which does what the instructions it stands for did, in their order.

Counted by the instructions the cases retire, a step of the particles is 3.35% fewer (its
`--p.life` was six instructions of the thirty-eight that a particle takes, and is two), and
nothing else moves: by the price list every operation is within two or three instructions of where
it was, the calls among them. On a quiet machine it was not timed either; with the two binaries
run in turn under the other work, the least of fifteen runs of a step of the particles is 5.1%
less.

`p.x` of a local that is pushed, as the left operand of an operator or an argument of a call is
(`p.x + p.vx`, `f(p.x, p.y)`), was the read and a Load. It is one instruction
(`LoadIndexPropertyWithCache`), one more kind in the arm that reads the property of a local, which
pushes what it found. The comparison with a constant that jumps (area 1) looked for a Load before
the constant, and would not have found a property that pushes itself: it takes back the push of
whichever instruction pushed the value now, as the operators with a constant do. So a variable of
an enclosing function and what an operator gives are compared with a constant by that one
instruction too (`count < 10`, `a + b < 10`), where they were pushed, the constant fetched and the
two compared.

Counted by the instructions the cases retire, a step of the particles is 1.5% fewer (thirty-two
instructions for a particle, from thirty-four), an animation by easings 1.6% and the properties of
one object read and written 1.2%; by the price list a call is two to three machine instructions
shorter, and nothing is longer by more than two. With the two binaries run in turn under the other
work, the least of fifteen runs of a step of the particles is 6.0% less, and of eleven runs of the
call of a closure 2.1% and of the easings 1.9% less. The table of the round's checks, one process
for a case under that work, is the first to have a step of the particles at 2x (1.96x, where the
one before it had 2.3x): a table of a quiet machine has to say whether it is under it.

`p.x = v` and `p.x += v` of a local pushed the object before the value was evaluated and popped it
where the assignment was made, which takes the object from the stack (`PutPropertyWithCache`).
Where nothing in the value assigns to the variable, the object is in the variable's slot when the
assignment is made as it was before the value, and the assignment takes it from there
(`PutIndexPropertyWithCache`): no push and no pop, one instruction fewer. The compiler asks the
references of the variable for a write inside the value; a function that the value calls has no
way to a variable in a stack slot, and a direct `eval` or a `with` in the function leaves the
variable no slot. A value that does assign to the variable (`p.x = (p = q, 1)`) is compiled as
before. undefined or null as the object throws after the value is evaluated, as it did, and a
variable that is not initialized yet before it.

Counted by the instructions the cases retire, a step of the particles is 3.4% fewer (thirty
instructions for a particle, from thirty-two), and the cases that call are 0.5 to 0.9% more. By
the price list a call is three to five machine instructions longer with this arm in the loop (479
for 476 with no argument), a getter four and a setter seven: the same with the arm at the end of
the loop's arms, and with a setter left to the general way and not called in place. It is what the
arm of the comparison with a constant cost (area 1), and what the property that is pushed gave
back. With the two binaries run in turn under the other work, in three pairs of fifteen to
twenty-one runs, a step of the particles is 0.1 to 3.4% less and the call of a closure 1.3 to 3.0%
more, which is more than its instructions say: both are to be taken again on a quiet machine
before this arm is counted as kept.

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

A spread element of a literal, and `concat`, still went the whole way of the specification for every
element (the profile of every case, area 8, showed both in `array/slice-concat-spread`: a third of
it each). A literal with a spread took each element of the iteration by CreateDataPropertyOrThrow: a
descriptor made of the value, the key and the length looked for, and the descriptor applied. `concat`
asked whether the source has each element, read it and defined it in the result, each by the
general way, 700 instructions an element. Both append now where that is all the steps come to. The
Array of a literal takes its next element as a plain one where it has no element descriptors and can
grow, which defining asks and no more (a prototype is not asked). `concat` appends the elements of a
source that is an Array with every element there as a plain value (no hole, under which the
prototypes are asked; no accessor, which is called) to a result that is such an Array as the literal
is, and as long as what was put in it so far. A source with a hole, an accessor or an element that
is not writable, a result that a species made otherwise (an object, an Array with an element
already, a frozen one, one that takes no properties or whose length is fixed), and whatever is not
an Array go through the steps as before.

`array/slice-concat-spread` retires 53% fewer instructions and is 0.68x for 1.6x with the two engines
run in turn; nothing else is changed. The test has each of those sources and results, the literals
with spreads between elisions and of other iterables, and a source that changes under the spread;
with any of the four things `concat` asks left out (every element there, no descriptors, the result
as long as what it holds, the result one that takes plain elements) it fails. The last of them did
not at first: the length set at the end threw for a frozen result as the first element should
have, and only a look at the result afterwards tells that nothing was put in it.

`map` and `filter` went on defining each element of their result by CreateDataPropertyOrThrow (the
profile of every case showed it in `array/map-filter-reduce`): a descriptor made of the value and
applied, to an Array that was made for the result a moment before. Both write the element now where
that is all the steps come to. The Array to fill is asked whether it takes plain elements, as the
result of `concat` is, and at every element, since the callback has run in between and can have
done anything to it; the index is then one of its elements (a hole of the `new Array(n)` that `map`
starts from, or a value that is plain as every element of such an Array is), which is written, or
the one after its last, which is appended. Anything else is defined as before.

`array/map-filter-reduce` retires 7.7% fewer instructions and is 1.64x with the two engines run in
turn; no call and no other case is changed. The test has results that a species makes (objects,
Arrays with elements already, frozen ones, ones that take no properties, with a fixed length, with
an element that is not writable) and callbacks that do each of those things to the result while it
is filled; with the question left out, with any of its three parts left out, or with an index past
the end taken for the one to append, it fails. The script for it found two defects that were there
before: an accessor element made a data element was left a hole, and an element made in
%Array.prototype% in place was not told to the caches (by `concat` of the round before too). Both
are with the defects, below.

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

Whether `push` simply appends (the Array can grow, and no prototype takes an assignment to an index)
was asked anew at every push, and two of its questions were dear. The Array was asked for its
prototype, which for an Array with no properties of its own made the realm look up the prototype of
its kind; and that answer, and the prototype of %Array.prototype%, were compared as objects of any
kind, a call each. An Array with no properties of its own takes new ones and inherits from the
realm's %Array.prototype%, so both are asked of the object that has its properties only where it has
one, and the prototypes are compared as what they are. And a `push` of one element made room for it
and then put it, which makes room itself. `array/push-then-pop` went from 10.24 to 9.20 ms with the
two binaries run in turn (1.79x to 1.64x).

`join` of an Array of Strings and Numbers (area 8) held its text to the limits of a string and of
the heap before every element, and that question goes to the host for what it has counted: a
twelfth of joining twenty thousand short pieces. It is asked once the text has grown by a page
since it was last asked, so the text may be that page longer than a limit before it is refused,
beside the one element it could be longer by before. With the push: `string/array-join-build` went
from 0.74 to 0.58 ms (2.11x to 1.65x), `array/join-numbers` from 3.50 to 2.94 ms (0.99x to 0.83x),
`string/split-join` 13% less. On the panel of 34 cases the times are 0.4% lower over all;
`typed/float64-math` reads 1 to 2.6% slower, by where the loop of instructions landed (the push is
part of the call arm, and six instructions left it).

`a[i] = v` looked the storage of the Array up three times: for its values with its descriptors, for
its descriptors, and again to write. A tenth of a loop that reads and writes the elements of one
Array. One function tests the element and writes it now, in one look: the element is there as a
value, and no descriptor says it is not writable. An element that has a descriptor and is still
writable (hidden, not configurable, in a sealed Array) is written in place as well, where any
descriptor sent the assignment the general way; a hole, an accessor, a read-only element and an
index past the length go the general way as before. `array/index-read-write` went from 7.59 to
7.24 ms with the two binaries run in turn (2.05x to 1.93x).

Only the arm for an assignment whose object and key are on the stack does this; the arm for an
assignment through a Reference (`a[i] += v`) looks three times still. With both changed, two calls
of a function were 3-4% slower (`call/plain-function`, `call/recursive-fib`), though neither writes
an element, and the way that was found is how such things are told apart since. The loop of
instructions is one function of 52 KB, and the compiler decides the places on the stack, the
registers and the order of the blocks for all of it at once: with 324 instructions taken out of
two arms, some 2,800 of its 13,135 were others (1,467 places), and with the one arm, 256 places.
So a case that runs none of the changed code can move by 3% either way, by more than where the
function starts. Branch targets put on 16-byte boundaries (an option of the compiler; the binary
5.6% larger) did not settle it: other cases moved instead. What is looked at is therefore a fixed
panel of 34 cases, each run on the two binaries in turn, and the mean of their changes, which was
0.2% lower with both arms and 0.6% lower with the one; and a change that touches fewer arms is
preferred.

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

A `for`-`of` over a String, a Map or a Set still left the loop of instructions at every step, for the
general way of the step instruction. That put the `Vm` aside for a call first, and then: for a String
Iterator with its original next took the step after all, and for the iterators of a Map and of a Set
called next and took apart the iterator result it made, the "done" and "value" of a new object for
each entry. The step is taken where that instruction is, before the `Vm` is put aside, as it is for
an Array and for the properties of `for`-`in`. The next of a Map Iterator and of a Set Iterator is its
step, which gives the entry, in an iterator result, and the instruction asks for the step where the
iterator has that next; one whose next a script replaced (in the prototype before the iteration
began, or on the iterator) goes the general way. `map/iterate` went from 15.83 to 11.30 ms with the
two binaries run in turn (1.63x to 1.19x), `string/code-points-iteration` from 1.05 to 0.81 ms
(1.92x to 1.49x).

Nothing of the loop of instructions is changed by this: the step is in the function the loop leaves
for, which costs it one leaving and no more, and the loop is the same code. No other case moved
(the five that the full run showed 3-5% slower are within 1.6% binary against binary). A step in the
loop itself would save that leaving and re-roll everything else (area 5).

A step of a generator that a loop resumes in place was 1060 instructions where QuickJS has 491,
half of them the generator suspended and resumed, and three things in that were not work.

The collections that the generator's instructions had scoped were released at every `yield`, and
the frames of a deep recursion let go of, each by a call, where there were none of either: asked
first now, as where a function is left (area 3).

A generator that yields in a loop has an exception handler between two steps, which every loop
pushes for its own cleaning up, and one that yields in a loop over an iterator has that iterator.
Each was handed over at the `yield` and again at the resumption as a part of a vector drained into
another, or appended to it: for one entry, the copying of some bytes by a call. One entry is moved
as the entry it is, into the room its vector kept from the step before.

And the `yield` went the general way of an instruction, which finds the instruction among all that
are not plain to say that it is a `yield`. The loop of instructions sees a `yield` itself now, and
suspends the generator where it had resumed it; a generator that was not resumed in place (one
stepped by a call of `next`) yields as before.

By the price list a step is 933 instructions for 1060: 56 less by the first two, 73 by the third.
The third is an arm more in the loop and a result more of an instruction, and it moved the rest by
where the compiler put things: a call is four instructions longer and a callback that a built-in
calls thirteen, a loop two shorter in every turn, the arithmetic of doubles three shorter.

With the two binaries run in turn, fifteen runs each, a step of a generator reads 9.4% less time
(`call/generator`, off 2x). A plain call, a method, a closure, a function that calls itself and
`new` are within 1.5% of where they were, as are the properties and the arithmetic; an empty loop
reads 3.7% more and a `switch` in a loop 2.8%, by where their code lies (the empty loop had read
6.4% less the round before for the same reason).

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

A String was quoted for JSON one code point at a time: each taken apart from its bytes, asked whether
it is one of the seven that have an escape of their own, a control character or a surrogate, and put
together again as bytes at the end of the text, a quarter of serializing a list of small objects
(the profile of every case, area 8, showed it). The code points that are not escaped go as the bytes
they are, so a text is copied by its stretches now: every byte goes as it is but those of the code
points that are escaped (below 0x20, the quotation mark, the reverse solidus, which are one byte
each) and 0xED, the first byte of every surrogate and of U+D000 to U+D7FF, and the code point at
each of those is looked at as before. `json/stringify` retires 6.8% fewer instructions and a list
of operations serialized 10.5%, with 12% fewer cycles each (0.89x and 0.81x with the two engines
run in turn). The test prints the code units of the
JSON text of every code unit below 0x100, of the code points around the surrogates, of pairs and
lone surrogates in every order, of escapes at the ends of a text and next to one another, and
reads every text back; with any of the four things the copy goes by left out (the surrogates' byte,
the quotation mark, the control characters, the three bytes that 0xED begins) it fails.

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

`a.length` of a TypedArray, which is in the test of nearly every loop over one, was nearly half of such
a loop (46% of `typed/uint8-fill-loop`, 13% beside the arithmetic of `typed/float64-math`). At every
read the shape of the array was asked for by its prototype (a table lookup: an array with no
properties of its own has no record of them), the property cache searched, the accessor read from
%TypedArray%.prototype through that object's descriptors (two more lookups) and compared with the one
the engine made; and then the length was computed by four of the specification's operations, each
reading the view and its buffer again.

That the property is that accessor is the same for every TypedArray of one kind that has no properties
of its own and the prototype of its kind, in one Realm, and it stays so until one of two things
happens, which the heap now counts. An object whose shape is changed in place (the intrinsic objects,
and every object that is a prototype) has a property added, removed or moved, or is given another
prototype: counted where the property caches are set right for it. Or a property that an object
already has is given a descriptor, or loses the one it had, which is the one way an accessor becomes
another accessor or a value with its shape, key and place unchanged: counted where an existing
property is overwritten. The loop remembers the kind and the Realm it last found the accessor for,
with the two counts and the count of collections (`TypedArrayLengthSeen`); while they stand, the
length is computed at once. An array with a property or a prototype of its own, and any array after a
change, goes the way it went. The length itself is one reading of the buffer and of the view, as the
validity of an index was already.

The readings of a view (its length, its buffer, the buffer's length) were functions of their own, and
an element read or written called four or five of them, each looking the view's record up again; they
are part of their callers now.

With the two binaries run in turn: `typed/uint8-fill-loop` 34.5 to 20.9 ms (2.04x to 1.18x),
`typed/float64-math` 16.5 to 13.3 ms (2.97x to 2.41x), `typed/pixels-rgba` 5% less (2.20x to 2.07x);
typed arrays from 2.3x to 1.85x, all cases from 1.519x to 1.503x. The loops and the calls are where
they were (each within 1.5%).

Tried: the validity of an index made part of the element read and write by force (it is a function of
its own that they call). The loops over typed arrays gained nothing more, pixels were 5% slower, and a
`switch` in a loop, which touches none of it, 8%.

Left for these: a double read from a `Float64Array` or made by an operator becomes a Value through a
function of some sixty machine instructions (whether it is an integer, whether it fits a Value, else a
Number of the heap), a fifth of `typed/float64-math`; and an element read goes through three functions
to its bytes.

Of that function: a double with any bit set in its low byte fits no Value, and is a Number of the heap
unless it is a whole number among the safe integers, which the conversion to an integer and back
tells. Those two tests are asked first now, where the function is called (not where an operator of
the loop has the conversion in line), and what they do not settle goes on as before.
`typed/float64-math` is 3% faster (2.41x to 2.31x), `math/random-and-date` 3%, arithmetic on doubles
in a loop 2%, trigonometry 1%. Tried: the whole conversion written as those tests (whole and safe, else
a low byte of zeros, else the heap), which gives the same Numbers for four million doubles and is
half as long. In line in the operators of the loop it was slower: trigonometry in a loop by 12%,
integers mixed with doubles and arithmetic on doubles by 8%.

Left: a double that is not an integer and has more than 45 bits of mantissa is a Number of the heap, one
allocated for the result of every operator, the intermediate ones too (a tenth of the loop, and the
collector's sweep of them a twelfth). A Value has seven bytes for what it holds; doubles in a Value
would take another layout of Value.

Where the elements of a TypedArray are was found anew at every element read or written: the record
of the array asked for its buffer, its offset and its length (each of which may be kept in a table
beside the record), the buffer for whether it is detached and how long it is, and the bounds of the
view worked out from those. All of it follows from the record, which is not changed once the array
is made, and from the length of the buffer in bytes. So what was found is kept, for four arrays at
once (`TypedArrayViewSeen`, in the heap): the array it is of, the collections that had run, the
buffer and its length, the offset of the first element and the number of elements. It is good while
no collection has run since (the handles name what they named) and the buffer is as long as it was;
a buffer that was resized, or detached, is not, and the view is worked out again. Making an array
forgets what was kept under its place. Nothing unsafe leans on it, and nothing could: the bytes are
read and written through the buffer's own checked accessors, and debug builds hold the place of
every element against IsValidIntegerIndex.

With the two binaries run in turn: `typed/pixels-rgba` 3.4% less, `typed/float64-math` 3.1%,
`typed/uint8-fill-loop` 2.1%; the panel 0.2% less over all. Less than the fifth of the pixel loop
that those functions were would suggest: what replaced them still looks at six things kept and at
the buffer's record, twice, for every element. Each part of what is kept in a cell of its own or all
of it in one made no difference.

What was left of an element read was not the look at what is kept. It was three functions one inside
the other: the one the loop of instructions calls, with a frame of six registers because the argument
of an arguments object is read by calls there; the choice of the kind of TypedArray, a jump through
a table to the function of that kind; and that function, with a frame of its own because the view is
worked out by a call where it is not the one kept. The element came back as a Numeric, to be made a
Value by the first. One function reads it now, for the nine kinds whose elements are Numbers: the
kind is chosen there, what is kept of the view is looked at, the bytes are read from the buffer and
the Number is made of them where it stands (a double that goes to the heap, where the heap's vector
of Numbers has room). It calls nothing and keeps no frame: the argument of an arguments object, and
everything of a TypedArray that is not read so (a view that is not the one kept, a BigInt kind), are
each a function of their own, called last. A write is the same, for a value that an element is made
of by a cast: an integer for the integer kinds, which is ToInt8 and the rest of it; an integer or a
double for the clamped kind and for the two kinds of doubles. Any other value, and a double into an
integer kind, is written as before.

The first try had the read and the write as part of the loop of instructions, where the compiler put
them, and every call was six instructions longer for it (the price list said so at once: the loop
had grown by the nine kinds twice). Each is a function of its own now, and the write is called from
what the loop had already.

An element of a `Uint8Array` is read in 162 instructions for 197 and written in 172 for 207, one of
a `Float64Array` in 231 for 271 and 172 for 191 (QuickJS: 89, 78, 106 and 76). Counted by the
instructions the cases retire, `typed/float64-math` is 6.8% fewer, `typed/pixels-rgba` 4.3%,
`typed/uint8-fill-loop` 4.0%; the calls are 0.2 to 0.4% more, by where the loop's arms came to lie.
With the two binaries run in turn the fill of a `Uint8Array` is 7.6% less and the arithmetic of a
`Float64Array` 1.4%; the pixel loop reads the same time for its fewer instructions (0.98 ms either
way, whichever is run first), as it did when the branch on a whole double was tried without a branch.
The test writes and reads every kind in loops with values of every sort, through two views of one
buffer, an offset and a subarray, past the length, through collections and with more arrays than
views are kept, and under a buffer that is resized; with any of the three things asked of what is
kept taken out (the array, the length of the buffer, the length of the view) it fails, once its
indices were variables: a constant index is another instruction, which had hidden two of the three.

Tried on the way and not kept: the choice between an integer Number and a double that fits a Value,
made without a branch. Samples of the pixel loop pile up behind the branch on whether a double is
whole, which is whole at every other pixel there, and that looked like a branch mispredicted half
of the time. Both made and one chosen, the loop was 4.8% slower with everything else at the same
addresses: the pile is what waits for the comparison, not a misprediction.

The arithmetic of doubles was the widest gap of the price list ("What an operation costs"): `x * 0.999 +
0.25` was 371 instructions, two operators of about 200, where QuickJS has 67. An operator of the loop
of instructions works out two integers in its arm and calls one function for everything else, and of
its 200 that function was 137: it took the operands apart as Numbers (a chain of conditional moves
over the three kinds, for each), asked what the operator is three times (a comparison or not; one of
the four of doubles or not; which), read the doubles, and called for the Number of the result, which
is 45 more. Now each operand is read as the double it is, once, the operator is found by one jump, and
the Number of the result is made there; the remainder, the shifts, the bitwise operators and operands
that are not Numbers are functions of their own, reached as before. The function is about 85, the
price of the two operators 269, and of `Math.floor(i / 3)` 666 from 744 (its division; and its read of
`Math`, area 2). Nothing in the loop of instructions is changed, which calls the function as it did.
With the two binaries run in turn (the function linked last in both, so that nothing else moves):
`loop/float-math` 21% less, `math/integer-to-double-mix` 15%, `math/trig-and-sqrt` 9%,
`typed/float64-math` 9%, an easing step 5%, `typed/pixels-rgba` 3%, `math/floor-round-minmax` 3%;
loops and calls on integers as they were, to the instruction.

Tried, twice, and not kept: a double held by the Value itself, whatever its fraction. Sixteen more
kinds of Value hold the doubles whose magnitude is from 2<sup>-63</sup> up to 2<sup>65</sup>: the tag
says the sign and the upper seven bits of the exponent, and the other 56 bits are the data.

As first built it ran every test and every script, and every price went up: the two operators to 485,
a call by 30 to 50, a typed array element by 30 to 60. A Number was sixteen bytes there (the double in
full, beside the integer of seven), which is returned through memory, its seven bytes stored one at a
time; the sixteen kinds were put together by a `match` over sixteen values that are not in a row, a
table of jumps; and their tags were above those of the objects, so that "a primitive" was no longer one
comparison of the tag anywhere.

Built again without the three (a Number of eight bytes with the sixteen kinds in it as a Value has
them, kind for kind; their tags right after those of the other Numbers, with the BigInts and the
objects moved up by sixteen, which by itself moves no price by more than 2; the kinds numbered in a
row), it ran every script again, and was no shorter and no faster. A standalone probe of the same
enum had made the two `match`es, a double into its kind and back, ten and nine instructions with no
branch. In the functions that use them the one that makes the kind is a table of jumps again: a
function with several ways out gives each arm its own return, and the compiler makes the addition only
of arms that all end in one place. As functions of their own, never read in, the two cost what they
replace: `x * 0.999 + 0.25` 271 instructions for 269. And the time was worse, with the two binaries
run in turn: `loop/float-math` 13% more, `typed/float64-math` and `typed/pixels-rgba` 11%,
`math/trig-and-sqrt` 10%, a step of the particles 6%.

The reason is that a Number of the heap is cheap here: 22 instructions to make (it is appended to a
vector of doubles) and seven to read, one of them a load, and the collector's share of a loop of
arithmetic is 3-6%. A kind per sixteenth of the range costs a test of the range, a test for a whole
number (which must be an integer Number, so that a Number has one form), and the taking apart and
putting together of tag and bytes, at every operator. So a double stays a Number of the heap unless
its low byte is zeros; what is left of the price of an operator on doubles is the instruction around
it (its operands fetched, the call) and the 85 of the function.

`Math.floor`, `Math.ceil`, `Math.round` and `Math.sign` asked their Number one thing after another:
whether it is integral, whether it is finite, whether it is a zero of either sign, and then for its
double. Each question looks at the Number's kind and, for a Number of the heap, reads it: five reads
for one `Math.floor(x)`. The double is read once now and the answers are taken from it (a zero, an
integral double and an infinity are their own floor, and the floor of a double between 0 and 1 is
+0: what the steps of the specification name one by one is what `floor` gives), and an integer
Number is returned before that. `Math.max` and `Math.min` of two integer Numbers, which is what most
calls give them, return one of the two after one comparison; the loop that takes any number of
arguments of any kind was eight times that. By the price list `Math.floor(i / 3)` went from 665 to
652 instructions and `Math.max(i, 7)` from 574 to 515, with nothing else moved. With the two
binaries run in turn `call/native-math` reads 5.0% less time and `math/floor-round-minmax` 5.1%; a
step of the particles, an animation's easing and a sum of integers, which call none of these, are
within half a percent of where they were.

The Number of a result is made without a frame (area 1, "The functions the loop calls"): the
function that works out an operator on doubles and the one that makes a Number for everyone else
each saved registers at every call for the one that grows the heap's vector of Numbers, and each
could fail in four places with a message. Where the vector has room, the Number is appended by
instructions that cannot fail; where it has not, a function called last grows it and makes the
Number. A double read from a Number of the heap whose handle names nothing is no double, and the
operands go the general way. `x * 0.999 + 0.25` is 241 instructions for 268, and `Math.floor(i / 3)`
624 for 652, with the read of `Math` and the entry of the call shorter too.

`-x` went the general way of an instruction for every kind of value. A Number is negated by a
function that the loop calls now (the negation of the double it is, made the Number that double is:
an integer stays an integer, zero is -0 and -0 zero); a BigInt and what is to be made a number first
go as before. `-(i & 7)` is 127 instructions for 181, `x = -x` of a double 115 for 202.

The operators of one operand were still far from what they do. `-x` was two instructions, ToNumeric
and UnaryMinus, and the negation made a double of an integer and a Number of the double again; `~x`
and `+x` went the general way every time, some two hundred instructions for a complement or for
nothing at all; and a numeric literal with a sign was no constant: `-1` was the constant 1 negated
where the code runs, every time it runs, so that `x === -1`, `return -1` and `i + -1` each paid for
a negation, and none of them was the comparison or the operator with a constant that the same
expression is with a literal that has no sign.

A numeric literal with a sign is a constant of the code now, Number::unaryMinus of it worked out
where it is compiled (`-0` is the constant -0; a BigInt literal is negated as before). `-x` and `~x`
are one instruction each, which is ToNumeric too: the loop runs it for a Number, and the general
way makes anything else a numeric value first and then does what it did. An integer that is not
zero is negated as the integer it is, a double that is kept in the Value by its sign (but -0,
whose negation is the integer zero), an integer complemented by its low 32 bits; a Number of the
heap is negated and any other double complemented as before. `+x` of a Number is its instruction
done in the loop, which is nothing.

By the instructions a million turns retire: `-(i & 7)` is 65 for 120 (QuickJS 22), `~(i & 7)`
64 for 211 (9), `+(i & 7)` 23 for 195 (9), `~~(i & 7)` 130 for 422 (19), `(i & 7) + -1`
65 for 201 (25); of a double, `-x` 65 for 110 (11), `+x` 24 for 195 (10), `~~x` 190
for 469 (97). No case of the suite is made of these, and none moves by more than its noise; by the
price list a call is two to five instructions shorter (559 for 562 with one argument) and nothing
is longer. The test has literals of every spelling with either sign and the zeros, each operator
and each two of them on values of every kind, the conversions in the order they are asked with
those that throw, the operands as locals, properties, calls and names not yet declared, and the
zero that the negation of -0 is, used as a key and compared. With any of these it fails: the sign
of a literal left out or put in, zero negated as an integer, an integer negated or complemented
wrong, -0 negated into a double, a text taken for a Number by `+x`, the complement of a double
taken as its negation, or the general way without its ToNumeric (where the engine stops at a value
it took for a numeric one).

What the operators of one operand cost was found by pricing them one by one, which the price list
had no rows for; the same was then done for eighty things that ordinary code does in every line (a
case file of loops of a million turns, each with one of them, run in both engines). Most are where
the price list says a thing of their kind is. The ones that were not, and are now: the length of a
String, the tests of a value that is no Boolean, and `undefined` (below, and in areas 2 and 8).
Those that still are not have rows in the price list from here on: `q++` and `++q` whose value is
used (189 instructions, QuickJS 18), `a || b` and `a && b` whose value is used (149 and 168 for 31
and 44), `o.x += 1` and `o.x++` (339 and 302 for 129 and 116), `a[i]++` (526 for 173).

ToBoolean asked a value that is no Boolean what it is five times over: whether undefined, null,
+0, -0 (each of the two asking whether it is a Number, and of which kind), NaN, the empty String.
It is one look at the kind of the value now: an integer is true unless it is zero, undefined and
null are false, a String kept in the Value is true unless it is empty, and only a double or a
BigInt is asked further. `if (x)`, `x ? a : b`, `!x`, `x || y` and `x && y` of an integer are each
ten instructions shorter (99 for 109 the `if` with its `else`); what is left of them is the jump
and the constant, as before.

`q++` whose value is used was seven instructions, of a variable in a stack slot too: the slot
read, ToNumeric, a copy pushed, the step, the slot written, the copy popped. Where the value is
not used it has long been one (`i++` as a statement). It is one now where the value is used as
well (`a[n++] = v`, `while (n--)`, `return ++count`): StepIndex, whose one argument is the slot
with the step and which value it leaves above it, and which the loop of instructions runs in the
arm of the step whose value is not used, for an integer that stays one; anything else is made a
numeric value the general way, once, and stepped there. `last = q++` is 69 instructions for 189
(QuickJS 17), and a `while (k--)` of three turns 925 for 1411 (277). A variable of an enclosing
function stepped for its value is as before (`a[n++ & 7] = i` with `n` outside, 454 for 83). The
arm asks which of the three it is by one look at the kind: asked first whether it is the new one,
before which way the old one steps, every `i++` that is a statement was a comparison longer, and
two loops of `while` with one in each 1.2% more instructions. As it is that case is 0.2% more, and
a call one machine instruction more by the price list (559.7 for 558.9 with one argument), which
is where the loop's code lies and no step of it.

The price list read every turn of a loop a machine instruction and a half longer with it, and with
an arm of its own for the instruction. It was neither: the list had grown by fourteen rows the
round before, and a case whose price is "the whole process, start included" pays for the
compiling of the file it is in, in both engines. A case priced over another does not, and those
were where they had been (a call with one argument 558.9 for 558.5). The test has each of the four
on values of every kind with the variable after it, the conversions once, the value as an index,
an operand, an argument, a test and a return, variables that are in no slot, declared later or
constant, the integers at their ends, and a function of more variables than the argument has bits
for, whose last ones are stepped the old way. With any of these it fails: the step the other way,
in the loop, in the general way or where it is compiled; the value after taken for the value
before, in any of the three; an integer left where it was past the largest, or the slot not
written; the step read a bit off the slot; a constant stepped; a slot too large for the argument
compiled so; the general way without its ToNumeric, and the instruction run as the step whose
value is not used (in both the engine stops). (The loops of the test are ended by counts that
are not themselves such a step: where they were, four of these never ended.)

`a || b` whose value is used was the left operand, a copy of it pushed, the test of the copy and
a jump on it, and then either the copy popped, `b` and a jump over what follows, or the copy
stored back as the result: four instructions where `a` decides and five where it does not, for an
operator that does nothing but choose. (In the test of an `if` or a loop the same operators are
jumps and nothing else, and were.) It is one instruction after `a` now: a jump over `b` that
leaves the value of `a` as the result where `a` decides the expression (it is true for `||`, not
true for `&&`, neither undefined nor null for `??`), and otherwise the register emptied for `b`.
`x = a || 9` is 71 instructions for 130 (QuickJS 27) and `x = a && 9` 91 for 169 (37); nothing
else is changed, by the price list or by the instructions that fourteen cases of every kind
retire (each within a thousandth).

These three have no general way: the loop runs them for any value and leaves them to nothing,
and what was written for them there at first was code that nothing ran, and that no test could
tell from its opposite. That came out of taking each piece out in turn, and with it that an Agent
which prints what it executes does not take the general way for every instruction, as the notes
of earlier rounds took it to: it reads each instruction with care and prints it, and the loop
then runs it as it runs any other. So a script run that way and compared with Node says that the
reading and the printing are right, and nothing of the general way, which only what the loop
leaves to it takes (a `typeof` of a Reference, a value that is no Number, a local that is no
integer: the tests of those rounds have each, and fail without it). A test of the engine now runs
one script in an Agent of each kind and expects of both what Node gives. The test of the
embedding has each operator for a left operand of every kind with whether the right one was
worked out, the values kept as they are, chains, what the right operand does and throws, the left
operand read once, and the value wherever a value goes; with either jump taken the other way, the
jumps of two operators exchanged, or null taken for a value by `??`, it fails. (The register
emptied where the jump is not taken is not seen by anything: `b` writes it next.)

`base ** exponent` left the loop for every pair of operands, and Number::exponentiate asked the
two Numbers what they are step by step (whether NaN, a zero of either sign, an infinity, an odd
integer: each step asking again of which kind the Number is, and reading a Number of the heap
again), made a Number of the absolute value of the base on the way, and made its result a Number
the slow way. `(i & 7) ** 2` was 537 instructions (QuickJS 248), `x ** 2` of a double 581 (278),
`x ** y` of two doubles 613 (184).

The loop calls one function for the operator now. Two integers give their product, by the
multiplications that the bits of the exponent ask for, where the exponent is not negative and the
product is a safe integer: that is what the power of the doubles is wherever a double holds the
result as it is. Any other two Numbers are read as the doubles they are, once; the steps of
Number::exponentiate are taken on the doubles (the same steps in the same order, the last of them
the `pow` of the C library), and the result is made a Number as a product's is. The general way,
which makes a Number of what is none first, takes the same steps on doubles. `(i & 7) ** 2` is 129
instructions, `2 ** (i & 15)` 169 for 545, `x ** 2` 283, `(i & 7) ** 0.5` 274 for 563, `x ** y`
286, `x **= 2` of an integer 220 for 628 (QuickJS 276).

The arm is a call and nothing else, as the negation's is. Written as one more of the operators of
the loop, with the power of two integers asked where they ask for their integers and the power of
two doubles where they ask for their doubles, it made the whole loop longer: nearly three
instructions a turn on a loop of three, 2.3% on the case that adds integers, 1.8% on the `switch`,
0.9% on the property reads, and six instructions on a product of two integers, which has nothing
of a power but the shape of its arm. As a call, nineteen cases of every kind retire what they did
(each within 0.12%; the empty loop and the tokenizer half a percent less).

Math.pow did not say what the operator says. Of two integers it multiplied step by step: exactly,
and rounded once, where the product is past the safe integers, which is the nearest double, and
where two are as near not always the one that `pow` gives (`Math.pow(3, 34)` was
16677181699666568 where `3 ** 34` is 16677181699666570, as in Node); and for a negative exponent
it took the reciprocal of a product of doubles, every multiplication of it rounded
(`Math.pow(10, -23)` was `1.0000000000000001e-23` where `10 ** -23` is `1e-23`; of the bases to
2000 with the exponents from -1 to -40, nearly half the pairs differed). It is the operator now:
the product of the integers where that is a safe integer, the steps on the doubles otherwise. Over
a grid of 641 values by 641 (the special values, integers and halves, the ends of 32 and of 53
bits, doubles of every size) the operator gives the bits it gave before, and Math.pow the bits of
the operator, where 8758 pairs differed. `Math.pow(x, 2)` of a double is 610 instructions for 734;
with a negative integer exponent it is 715 for 604, which is what the operator's answer costs.
(That answer is not everywhere the nearer one: `pow` is within a unit of the last place and not
always the nearest double, so that `9007199254740991 ** -1` is 1.1102230246251565e-16 where
`1 / 9007199254740991` is 1.1102230246251568e-16, and 31 of the reciprocals of the integers to
20000 differ so, in Node the same 31. Math.pow gave the division's there, and gives the
operator's now.) What is left of both is the call: `Math.sqrt(i & 7)` is 428 instructions (QuickJS 249), of which the read
of `Math` is some 90, of its method some 150 and the call some 190.

What a script can expect of a power is less than it seems. A result that no double holds is
rounded by the C library, and where two doubles are as near each library chooses for itself; so
the test asks for such results within a distance of what they are, for the rest as Node prints
them, and that Math.pow gives what the operator gives. It has small integers with their signs,
negative exponents, zero and one to exponents past 32 bits, results at the end of the safe
integers and past it to the infinities, both zeros, both infinities and NaN as either operand one
by one and as a table of nineteen values by nineteen, 1 and -1 to an exponent that is not finite,
the result used as an index, a length and an operand, operands that are converted, BigInts and
Symbols, and the forms the operator is written in. With any of these it fails: a negative exponent
or a product past 64 bits or past the safe integers taken for an integer power; the steps for a
NaN exponent, a zero exponent or a NaN base left out (in a build that asserts, as the tests are:
the assertions of steps 8 and 11 stop the engine there; without them the first two are wrong
answers, and the third is what `pow` says too); a wrong answer in any of the steps for an
infinite or a zero base or an infinite exponent, each taken in turn, or an exponent that is no
integer taken for an odd one; every negative base given NaN; and the operands exchanged, in the
loop, in the general way or in Math.pow. Two things it cannot tell: step 12 left out (a negative
base with an exponent that is no integer, which `pow` answers NaN as well here), and the arm of
the loop leaving every power to the general way, which gives the same. The steps that `pow`
would also answer stay as the specification has them, for a library that answers otherwise.

The quotient of two integers was a quotient of doubles, whatever the integers: the two were made
doubles and divided, and the result was asked what every double is asked (whether it is a Number
of the heap, whether whole, in the range of the integers, not -0) before it was the integer that
`n / 2` is for every other `n`. `i / 1` was 76 instructions more than `i & 7` (QuickJS 12),
`(i & 6) / 2` 138 more (33), `(i + n) / 2` 162 more (41).

Two integers that divide give their quotient as the integer it is now, where the division is one
of the instructions that have the operator as an argument (a local or the value just worked out,
by a constant or by the value just worked out: `i / 2`, `(a + b) / 2`, `n / (i + 1)`, `x /= 4`).
Those ask whether their operands are two integers whatever the operator is, and the division was
the one operator that then had nothing for them. A zero divisor, two integers that do not divide,
and zero by a negative divisor, which is -0, go on to the doubles as before. A division that comes
out even is 9 instructions more than `i & 7` for 76, and one that does not 6 more than it was (the
division that was tried): `i / 2` 41 for 72 with `i` even and odd in turn, `(i & 6) / 2` 70 for
138, `i / 3` 54 for 73, `(i + n) / 2` 129 for 162, `(i & 7) * 5 / n` 154 for 224,
`Math.floor(i / 3)` 476 for 495.

Three other shapes were measured and dropped, and each says something of the loop. A quotient that
does not come out even, kept in the Value where its bits are few (the halves and the quarters),
made in the same place: `i / 2` was 18 instructions; but a second kind of value leaving the code
that all the operators on two integers share made each of them two to four instructions longer
(`i & 7`, `i + n`, `i * n`), and a loop of nothing else a hundredth. The two integers divided in
the function that operates on doubles, in its arm for the division: nothing else moved, and the
division was eleven instructions longer than it had been (83 for 72), the function keeping a
frame where it kept none. And the same try in the arm of the division that has no operator as an
argument (`a.x / b.y`, `0.25 / (i + 1)`): every division of doubles there was nine instructions
longer, and every one of two integers that do not divide fifteen. That arm asks for nothing but
the doubles, as it did before (`divide_numbers`); a case that divides a double in every turn
would have been 1.5% longer.

As it is, twenty-one cases of every kind retire what they did (each within 0.17%), and of the
loops of one operation only `i + 7` is longer, by one instruction of 182. The test has each of
the four instructions with pairs of every kind (integers that divide and that do not, of every
sign, the ends of the safe integers, zero as the dividend and as the divisor, doubles, the
infinities, NaN, -0 and what is no Number), the quotient used as a key, an index, a length and
a count, and found as the Number it is in a Map, a Set and an Array, and two grids, one against
the remainder and the product and one against the same division made of doubles. With any of
these it fails: zero by a negative divisor given as the integer zero, or zero by a positive one
left to the doubles in its place; two integers that do not divide given their quotient cut short;
the operands exchanged, among the integers or among the doubles; the quotient with its sign
changed; a zero divisor not asked for (the engine stops). Which way a division goes it does not
say, and cannot: with the try at two integers taken out, or put into the arm that has none, it
passes, the quotients being the same.

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

The `+` of two operands still took the general way of an instruction whenever one of them was a
String: `s += "…"`, `"<td>" + cell`, `text + i`. The loop left, the instruction was read again with
what it is fused with (the local, the constant), and the operands were asked whether they are Numbers
before they were joined: a sixth of a loop appending short texts, on top of the appending itself. It
is done where the operators have their one call for Numbers that are not two integers
(`Vm::float_operation`). Operands that are not two Numbers are, for `+`, a String and a String or a
primitive that ToString takes without running anything (a Number, a Boolean, undefined, null, a
BigInt; not a Symbol, which it throws for): made text and joined as the general way joins them,
after the same look at the length of the result, which is what holds a text to the limit of a
string and of the heap. Where that look refuses, and for an object, nothing is done there and the
general way does as it did.

With the two binaries run in turn: `string/append-100k-char` went from 6.02 to 4.99 ms (1.97x to
1.63x), `string/append-20k-short` from 1.19 to 1.05 ms (1.79x to 1.52x), text outside ASCII appended
12% less (1.09x to 0.95x), `string/prepend-5k` 11% less (1.89x to 1.67x), Numbers appended and rows
of HTML 3% less. The loops, the calls and the properties are within 1.6% of where they were.

That last sentence took three ways of writing it. A call of its own for the Strings, in each arm of
the loop that has an operator, made the loop of instructions 392 bytes longer, a loop adding
integers 9% slower and a `switch` 6%. The arms cut down to the integers and the one call, which then
tells Numbers from the rest itself, made it 1048 bytes shorter, the loop adding integers 3% slower
and one of doubles 4%. What is kept leaves the arms as they were but for one thing: operands that
are not Numbers, under an operator that is no comparison, go on to the call that was there, where
they left at once.

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

Interning a substring did more than it had to, twice over. The piece was copied out of its source
into a buffer of its own, the string table was asked for that text, and where a string had it
already, which is the common case in a loop over one text, the buffer was thrown away. And the
length of a piece that is not ASCII was answered by making its table of indices: an allocation of
eight bytes for every code unit, filled by a walk over the text, for a piece whose length is all
that is ever asked of it.

- A substring whose bytes lie in its source as they are (no half of a surrogate pair at either end)
  is looked up in the string table where it lies, and copied only if no string has its text
  (`Heap::alloc_wtf8_slice`): `slice`, `substring`, `substr`, the pieces of `split`, what a RegExp
  matched and captured.
- A text of up to 64 bytes that has no table of indices is counted for its length, in one pass
  over its bytes. Asked for an index it gets its table as before, and its length is then read from
  that. A longer text gets the table at once, so that a loop asking a long
  text for its length at every turn does not count it at every turn.

With the two binaries run in turn: `string/slice-non-ascii` 11-13% less (2.18x of QuickJS to 1.9x),
`string/split-join` 22% less, `string/slice-substring` 6-8%, `string/code-points-iteration` 5%.

`a < b` of two Strings left the loop of instructions for the general comparison, and that decoded both
texts into UTF-16 code units one code point at a time to compare them. The four comparisons of two
Strings are answered in the loop now, and the texts are compared as the bytes they are kept as: the
common beginning is passed over eight bytes at a time, and the first bytes that differ decide when both
are below 0xED, where the order of WTF-8 bytes is that of code units (it is not only where a surrogate
pair meets U+E000 and up, and those are decoded as before; a test compares every pair of a set of texts
around that boundary with the order of their code units). `string/equality-and-compare` went from 2.15x
to 1.8x, sorting objects by a number and then a name from 1.85x to 1.6x.

Whether two Strings are the same was still asked the whole way of the specification: whether the two
values are of one type, whether either is a Number, whether either is null or undefined or a BigInt,
and then whether both are Strings, each of those a look at both values. A profile of every case, one
after the other, for the function with the most samples that is not the loop, showed it three times:
88% of an Array of Strings searched by `indexOf` and `includes`, a third of a loop that compares
each character of a text with five vowels, a tenth of a text escaped character by character. Two
Strings are answered before any of that now, by `===`, by SameValue and by SameValueZero: two that
are kept in their Values are the same where they are the same bytes, one of those is never the same
as a String of the heap (which is never as short: the comparison at the end of the steps says the
same), and two of the heap are the same where they are one and are compared by their text where
they are not. It is in the three functions and not in the loop, which is as it was.

`array/index-of-includes` retires 59% fewer instructions and is 0.67x for 1.7x with the two engines
run in turn; `string/bracket-index-ascii` 17% fewer (0.66x for 0.77x), `string/equality-and-compare`
12% (1.11x), a text escaped by its characters 8% (0.89x); the calls are the same. The test compares Strings of every
length around the seven bytes a Value keeps, each made in five ways, with the literal, and every
pair of them, by every way of comparing; a wrong answer for two short Strings or for a short one
and one of the heap fails it. For two of the heap no script tells the comparison of the texts from
none: two with the same text were one String in everything it made.

The same profile showed `instanceof` reading the function's "prototype" by Get, as `new` did (area
3), and it reads it by its cache now; the case that tests it is 1% fewer instructions, so that was
not what it spends.

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

`match` with "g" among the flags goes the same way since: for a RegExp with that built-in protocol the
matches are found by the matcher alone and the Array is made of their texts, where RegExpExec made a
result at every match for its "0" to be read. The loop that collected the ranges for `replace` gives
each match to its caller for this, so the texts are kept as they are found: a list of ranges first
would be several times what the Array takes, and a text matched at very many places would be refused
under a heap limit where it was not.

And a result of RegExpBuiltinExec, which every `exec`, every `match` without "g", `matchAll` and
`split` still make, was an Array given "index", "input", "groups" and each capture through
\[\[DefineOwnProperty]], one at a time: a third of a loop over the matches of a RegExp. Where no
capture has a name it is made at once: the captures are the elements of the new Array, and the three
properties, in that order, are what the ordinary object that has an Array's properties is made with,
where the steps made it empty for the first and added each. A setter put where those properties
would be inherited from is not called either way: they are defined, not assigned.

With the two binaries run in turn: `regexp/exec-groups-global` went from 1.28 to 1.02 ms (2.21x to
1.87x), `regexp/split-and-match` from 7.93 to 6.10 ms (2.18x to 1.75x); the loops, the calls and the
properties are within 1.6%, `loop/switch` 2.3% slower with the loop of instructions the same code
132 bytes further on. A hundred thousand matches of one character in one text take 6 ms by `match`
and 26 ms by a loop of `exec`, where the build this started from took 29 and 46, and Node takes
under 1 and 4: what is left is in each search before the matcher runs (the heap measured for the
limit, the room for its stacks worked out, the text looked at) and in the call of `exec`.

`Array.prototype.join` was the specification's loop for every Array: for each element the separator read
from its rooted handle, Get of the element by its index through the general way of a property, a
comparison with the Array, ToString by the general way, and the handle of the Array read three times.
Where the Array has no element descriptors, its elements that are Strings, Numbers, Booleans, undefined
or null are read from where they are and appended, for as long as there is nothing else: Get of such an
element is the element, and ToString of it runs nothing, so that the Array is what it was at the next
one. A hole (which a prototype may answer for), an object, a Symbol and a BigInt go on from where they
are by the loop as it was, with the text so far. The host is asked and the length of the text checked at
every element, as before. `parts.join("")` of the pieces of a text is this path: numbers joined went
from 4.75 to 3.34 ms (1.36x to 0.96x), a text split and joined from 1.18x to 1.02x, strings pushed and
joined from 0.83 to 0.74 ms (2.29x to 2.11x; the rest of that case is `push`).

The method of a String or a Number (`s.slice`, `s.charCodeAt`, `n.toFixed`) is found by the cache under
the shape of the prototype object of its kind, and was then read from that object's storage by three
general functions; it is the one function that reads a property of an ordinary object by its cache
now. The cases that call such a method in a loop are 1-4% faster (`string/char-code-at-non-ascii`
3.7%, `string/slice-non-ascii` 2.3%). As part of every arm that reads a property (it was in line) the
same change made the calls 1-4% slower by where the loop's code lay; it is a function of its own.

Tried: the method of an Array (`a.push`) found under a remembered shape of the Arrays that have no
properties of their own, and read from its slot. The case of strings pushed and joined gained
nothing (what `push` costs is in `push`: whether the Array appends plainly, its storage grown, the
bytes counted), and a plain call and the properties of one object read 4-6% slower. Dropped.

`s.repeat(n)` and the filler of `padStart` and `padEnd` were appended a copy at a time, each the way
a text that may end in half a surrogate pair is appended: the end of what is there and the start of
what comes are looked at, so that two halves that meet become one character. That was two thirds of
`"-".repeat(n)`. A text that is valid UTF-8 has no such half, and says so in one pass over its bytes:
its copies are then its bytes, as many times, and only a text with a lone surrogate in it is
appended the careful way (`"\uDC00\uD800".repeat(2)` has a pair at its seam). A result that is too
long for a String or for the heap is refused before any of it is made, as it was.
`string/pad-and-repeat` executes 6.8% fewer instructions and reads 7.5% less time, the two binaries
run in turn.

`s.length` counted the String at every read. A text of up to 64 bytes that was never asked for
an index has no table of its indices (which is eight bytes a code unit, made for a text that is
indexed), and its length was counted from its bytes each time, in "a few instructions" that were
six a byte: 190 instructions for eleven characters and 417 for sixty, at every turn of
`for (i = 0; i < s.length; i++)`; a string kept in the Value was looked through twice, a byte at
a time, for 116. A short text of ASCII is found to be one by words now, and remembered to be one:
that is all there is to know of its indices, and nothing is allocated for it. A text whose
indices are known says its length by them before anything else is asked. A string kept in the
Value is looked at as one word: the first byte with its high bit is where its padding starts if
it is ASCII, and it is ASCII where everything from there on is padding. The length is 74 to 76
instructions whatever the text (QuickJS 7; an Array's is 50), but for a short text that is not
ASCII, which is still counted (225 for eleven characters) and waits for somewhere to keep the
count.

The test of the three (the length, the tests of a value, the three values of the global scope)
has Strings of every make and length around the limits, with what is read of a string after and
before its length; what is true of sixty values of every kind in each test and loop; and the
three values compared, shadowed in every way, read in code of `eval`, assigned to and used as
defaults, keys and cases. With any of these it fails: zero, null, the empty String, NaN, -0 or
the BigInt zero taken for true; a short text taken for ASCII unseen, or the length of one that is
not taken from its bytes once its indices are known; a string kept in the Value taken for ASCII
unseen, or its padding found a byte late; `NaN` or `undefined` made another of the three. (Where
a length comes out wrong the engine stops at the next thing that is cut by it.)

### 9. Garbage

Allocation-heavy loops are about 2x, strings as garbage 5x. Not a priority.

A collection looked at every item of every vector of the heap to keep it or drop it, the mark of
each taken from an iterator over the marks, one at a time. A loop of arithmetic on doubles leaves
some hundred thousand dead Numbers between two collections (every result that is no small Number
is a Number of the heap, area 6), and the look at each was 5.6% of the time of
`math/trig-and-sqrt`. The marks are read 64 at a look now and only the live items are visited:
each changes places with the first dead one below it, which keeps the live ones in their order,
and the dead ones are dropped together at the end. A run of dead items costs its words. The sweep
is 1.7% of that case after, `x * 0.999 + 0.25` on doubles is 221 instructions for 236, and counted
by the instructions the cases retire trigonometry and the arithmetic of a Float64Array are 2.4%
fewer, a step of the particles and a frame of easing 0.3%; a tree built and walked and a
tokenizer, where most of what is swept lives, are the same to 0.2%.

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
- A function of its own that an arm of the loop of instructions calls has to be given only what fits
  registers. One that took the loop's tables (three slices, passed by address), tried for running a
  property read with the instruction that fetches its object, made every loop and call 17-34% slower,
  though none ran it: the tables were kept in memory from then on. Given the cache and the key it
  cost nothing elsewhere, and gained nothing where it was for (area 1).
- `typeof K` before `class K {}` in the same function gives "undefined" where it is to throw a
  ReferenceError (the class is in its temporal dead zone), here and in the build this started from;
  `typeof` of a `let` or a `const` there throws. Seen in the script for `typeof`, which leaves it
  out. Not changed.
- `ArrayBuffer.prototype.transfer` is there and throws "not implemented", here and in the build this
  started from; `o[key] += v` makes a property key of `key` once, where Node does it twice (the
  specification says once). Seen in the scripts for the length of a TypedArray and for compound
  assignment, which leave both out. Neither is changed.
- An Array has a slot for every index up to its length: `a[4294967294] = v` asks for all of them, and
  under a heap limit the script ends for it (out of memory), here and in the build this started
  from, where another engine keeps such an Array sparse. Without a limit the slots are asked for and
  never touched. Seen in the script for the writing of elements, which leaves it out. Not changed.
- `x instanceof p`, where `p` is a Proxy of a function, is false for an `x` the function made, here
  and in the build this started from (and by the general way, with the loop's own answer taken
  out); it is true by the specification and in Node. Seen in the script for `instanceof`, which
  leaves it out. Not changed.
- An element of an Array that is an accessor, defined again as a data property with every attribute
  true, was left a hole, here and in the build this started from: such an element has no
  descriptor, so the accessor's was taken away, and the value was not put in the slot.
  `Object.defineProperty(a, 0, { value: 1, writable: true, enumerable: true, configurable: true })`
  does it, and so does CreateDataProperty over an accessor, as `map` does where the callback made
  one in the Array a species gave. Seen in the script for `map` and `filter`. Fixed: the element is
  given its fields as a data element is.
- %Array.prototype% is an Array, and a new element of it has to be told to the caches of property
  reads that did not find its key there; the steps that define an element do that. What writes an
  element in place did not: `copyWithin` copying a value to a hole, here and in the build this
  started from, and `concat` since the round that made it append (a species can give
  %Array.prototype% as the Array to fill; `map` and `filter` had it too until the script for them
  asked). After it a function that had read `x[0]` of an object that inherits from
  %Array.prototype% went on reading undefined. `unshift` had the same in the build this started
  from, and not here since it asks whether the Array appends plainly. Fixed: an intrinsic Array
  takes no element in place (`Array::is_intrinsic`), in `takes_plain_elements` and in `copyWithin`;
  the test has every way an element of %Array.prototype% is made.
- The `with` statement is not there, here and in the build this started from: its object is
  evaluated and made an object, and the body then runs with no Object Environment Record made of
  it, so a name in the body is never a property of the object (`with ({ x: 9 }) { x }` is a
  ReferenceError, or the `x` outside). Strict code, which the engine runs by default, has no
  `with`. Seen in the script for `undefined` as a constant, which leaves it out. Not changed.
- `Math.pow` of two integers was not `**` of them, here and in the build this started from: it
  multiplied step by step, so that a product past the safe integers was rounded another way than
  the power of the C library rounds it (`Math.pow(3, 34)` 16677181699666568, `3 ** 34`
  16677181699666570), and a negative exponent gave the reciprocal of a product of doubles
  (`Math.pow(10, -23)` was `1.0000000000000001e-23`). Seen in the script for the power, which asks
  that the two agree. Fixed: Math.pow is the operator (area 6).

## Checking a change

`cargo test -p nova_vm -p nova_embed -p nova_embed_ffi -p wtf8`, the widget sandbox's own suite against the built
library (224 tests in process, over the stream and hosted, the attack corpus, the compat corpus), then
`safe/bench/run.py --baseline …` for the numbers, and test262 as `safe/tools/gate.sh` runs it (no unexpected
result; 13 or 14 tests that timed out now pass, depending on the run).
