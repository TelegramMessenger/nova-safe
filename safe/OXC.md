# The parser stack without `unsafe`

Goal: no `unsafe` in any crate that parses a widget's source text. The parser is the largest remaining attack surface:
a widget controls every byte it reads. Rust's std, including its HashMap (hashbrown), stays trusted.

## Where it stands (phase 0)

- oxc 0.124.0 is vendored in `oxc/crates/<name>` (MIT, `oxc/LICENSE` and one copy per crate), with its tests,
  benches and dev-dependencies left out. The workspace points at the paths and excludes `oxc/` from its members,
  so `cargo test --workspace` does not build oxc's unit tests. Vendored: `oxc_allocator`, `oxc_ast`,
  `oxc_ast_macros` (proc macro), `oxc_ast_visit`, `oxc_data_structures`, `oxc_diagnostics`, `oxc_ecmascript`,
  `oxc_estree`, `oxc_index` (4.1.0), `oxc_parser`, `oxc_regular_expression`, `oxc_semantic`, `oxc_span`, `oxc_str`,
  `oxc_syntax`. To redo the copy: `vendor.py` in the phase-0 scratch dir (copies from `~/.cargo/registry`, adds the
  path entries, drops dev-dependencies and examples except the ones that docs `include_str!`).
- Changes against upstream so far: `oxc_diagnostics` has a `fancy` feature (off by default; `nova_cli` turns it
  on for its terminal report), `oxc_span` no longer asks for miette's `fancy-no-syscall`, `oxc_syntax` and
  `oxc_semantic` no longer turn on `oxc_index/serde`, and two deprecated `max_value()` calls became `MAX`.
  That removed 24 crates and 567 `unsafe` from the widget build: textwrap, owo-colors, smawk, smallvec (80),
  serde and serde_core, and the icu/zerovec family (zerovec 236, yoke 90, utf_types 31, stable_deref_trait 25,
  tinystr 22, icu_provider 19, utf8_iter 16, icu_collections 15, potential_utf 10, zerotrie 7, icu_locale_core 6,
  writeable 3, icu_segmenter 2, icu_locale_fallback 1, litemap, zerofrom and the icu data crates). Widget build
  before: 3235 `unsafe` (TCB.md's 3518 less the regex crates that regress replaced); after: 55 crates, 2668.
- `python3 -I safe/tools/dep_unsafe_report.py . [--markdown] [--default-features]` counts `unsafe` in every crate
  the widget build links (`cargo tree -p nova_vm -e normal,no-proc-macro --no-default-features --features
  widget`), marks each crate `registry`, `vendored` or `workspace`, and prints the total with and without std's
  hashbrown. Same counting rule as TCB.md (keyword outside comments, `tests/` `benches/` `examples/` skipped,
  code for other targets counted).

## Phase A (2026-11-14): the safe allocator

Done, each step through the gate:
- The nesting pre-scan now counts a call, an index or a tagged template after an operand as one more level. Before,
  `f` + `()` x 50000 (likewise `a[0][0]...` and chains of tagged templates) passed the pre-scan and overflowed the
  native stack of a release build in the semantic checker and the compiler, aborting the process. New tests: the
  pre-scan counts the three chains; `nova_embed` parses, compiles, runs and drops the deepest tree of 17 shapes the
  pre-scan lets through on the widget engine's 8 MB stack (64 MB in an unoptimized build: its bytecode compiler
  frames overflow 8 MB at the deepest member chain, with or without this change).
- nova's `parse_program` returns the `Program`; the caller keeps it in a local and `check_program` builds the
  semantic data from a borrow of it (no `Allocator::alloc`).
- `Visit<'a>` (oxc_ast_visit) and the regular-expression `Visit<'a>` take `&'a T`; the lifetime-transmuting
  `alloc()` is gone and the semantic builder uses the references it gets. The serialize-only UTF-8 to UTF-16
  converter is removed. oxc_ast_visit and oxc_regular_expression forbid `unsafe_code`.
- The lexer pushes legacy octal escapes as `char`s.
- oxc_allocator is the safe design below, with `#![forbid(unsafe_code)]`: 273 -> 0. `Vec::push_fast` is kept as
  `push`. oxc_ast's generated `escape_template_element_raw` builds a `String` instead of writing through a raw
  pointer, and the `discriminant!` const assertions are gone (they built enums around `Box::dangling()`).
- Vendored oxc: 707 -> 407 `unsafe`; widget build 2668 -> 2368 (1585 without std's hashbrown). allocator-api2
  stays in the tree only through nova's own hashbrown 0.17 default features (the dependencies track).

Parse plus semantic check plus drop (`parsebench`), release, best of N, same machine and run:

| input | arena | safe allocator |
|---|---:|---:|
| sandbox-bundle.js (598 KB) | 7.0 ms | 11.3 ms (+60%) |
| 07-shop-06-shop-bare | 0.069 ms | 0.105 ms |
| 08-analytics-10-bot-bare | 0.090 ms | 0.135 ms |
| 12-motion-lab-02-mo-bare | 0.073 ms | 0.110 ms |
| hostile/text-chaos | 0.015 ms | 0.024 ms |

Same symbol and node counts. A widget's own scripts cost about 0.1 ms to parse against a median widget run of
about 150 ms; the bundle is parsed once per process.

`AstKind::ty` is a generated `match` (188 arms) instead of reading the tag through a pointer, and
`RegExpFlags::to_inline_string` uses `InlineString::push` (checked) instead of `push_unchecked`.

Left in oxc_ast (2): `shared_enum_variants!`'s `as_*`/`as_*_mut` (and the `to_*` that call them) cast `&Parent` to
`&Child` (`&Expression` to `&MemberExpression`, `&Argument` to `&Expression`, `&Statement` to `&Declaration`, ...).
The casts are sound while both enums are `#[repr(C, u8)]` with equal discriminants, which the removed assertions
checked and the explicit discriminants in `inherit_variants!` still give, but they are the next thing to go. Callers today (the build tells them all once `as_*` is removed): oxc_ast_visit's walks (18), oxc_ast
itself (22), oxc_semantic (3), oxc_ecmascript (40, mostly `as_expression`/`to_expression` on arguments and
array elements), nova (about 40). Two ways, both mechanical but wide:
- nesting: `Argument::Expression(Expression<'a>)` instead of copying Expression's variants into Argument, so
  `as_expression` is a `match` returning the inner reference. Every direct pattern on an inherited variant
  (`Argument::Identifier(..)`) and the 48k lines of generated derives (`oxc_ast/src/generated/`) change; without
  oxc's `ast_tools` generator that is a scripted rewrite of the generated files.
- dispatch: keep the flat enums, remove `as_*`/`to_*`, and give callers a `match` that dispatches each inherited
  variant to the per-node code (a `match_expression!`-style macro with a body), plus borrowed view enums where a
  function needs "any member expression" (`visit_member_expression`, the semantic builder's two overrides).
  nova's compiler already compiles per node type, so its `as_expression` sites become such a dispatch; oxc_ecmascript's
  constant evaluation on `&Expression` is the hard part (it needs `&Expression` from an argument) and may be cut
  instead: check which of its traits nova calls.

### Widget build, after phase 0

| crate | version | source | unsafe | area |
|---|---|---|---:|---|
| hashbrown | 0.17.1 | registry | 454 | trusted (std) |
| memchr | 2.8.3 | registry | 333 | F |
| hashbrown | 0.16.1 | registry | 329 | trusted (std); A moves oxc to 0.17 |
| oxc_allocator | 0.124.0 | vendored | 273 | A |
| allocator-api2 | 0.2.21 | registry | 214 | A (only oxc_allocator and hashbrown's `allocator-api2` feature need it) |
| oxc_parser | 0.124.0 | vendored | 170 | B |
| compact_str | 0.9.1 | registry | 159 | C |
| oxc_data_structures | 0.124.0 | vendored | 151 | C |
| soavec | 0.2.0 | registry | 151 | not this track (the heap's storage) |
| oxc-miette | 2.7.1 | registry | 89 | D |
| castaway | 0.2.4 | registry | 53 | C (comes with compact_str) |
| self_cell | 1.3.0 | registry | 34 | C |
| oxc_index | 4.1.0 | vendored | 29 | C |
| ryu | 1.0.23 | registry | 27 | E (comes with compact_str) |
| dragonbox_ecma | 0.1.12 | registry | 20 | E |
| oxc_ast_visit | 0.124.0 | vendored | 20 | G |
| oxc_semantic | 0.124.0 | vendored | 18 | G |
| oxc_estree | 0.124.0 | vendored | 17 | G (drop the crate) |
| itoa | 1.0.19 | registry | 16 | E (comes with compact_str) |
| siphasher | 1.0.4 | registry | 12 | F (phf) |
| foldhash | 0.2.0 | registry | 10 | trusted with hashbrown |
| itertools | 0.14.0 | registry | 10 | F |
| nonmax | 0.5.5 | registry | 10 | C |
| oxc_ecmascript | 0.124.0 | vendored | 10 | G |
| num-bigint | 0.4.8 / 0.5.1 | registry | 8 + 8 | F |
| cow-utils | 0.1.3 | registry | 7 | F |
| oxc_syntax | 0.124.0 | vendored | 7 | G |
| oxc_ast | 0.124.0 | vendored | 6 | G |
| oxc_str | 0.124.0 | vendored | 5 | C |
| unicode-normalization | 0.1.26 | registry | 5 | F |
| percent-encoding | 2.3.2 | registry | 4 | D (only oxc_diagnostics) |
| bitflags | 2.13.2 | registry | 2 | F |
| either | 1.19.0 | registry | 2 | F (itertools) |
| unicode-id-start | 1.5.0 | registry | 2 | F |
| num-traits | 0.2.19 | registry | 1 | F |
| oxc_regular_expression | 0.124.0 | vendored | 1 | G |
| static_assertions | 1.1.0 | registry | 1 | C (compact_str) |

The other 18 crates have none (cfg-if, equivalent, num-integer, oxc_diagnostics, oxc_span, phf, phf_shared,
rustc-hash, thiserror, tinyvec, unicode-segmentation, unicode-width, xsum and the four workspace crates).
Vendored crates: 707. Registry crates: 1961, of which 783 are std's hashbrown.

## Area A: the allocator (decides the rest)

Files: `oxc_allocator/src/*` (273: `bump.rs` 61, `vec2/` 68, `string_builder.rs` 43, `bumpalo_alloc.rs` 15,
`allocator_api2.rs` 9, `alloc.rs` 11, `allocator.rs` 8, `boxed.rs` 8, `clone_in.rs` 8, plus `pool/`,
`from_raw_parts.rs`, `bitset.rs`, `tracking.rs` that nova does not compile).

A bump arena that hands out `&'a T` for any `T` cannot be written in safe Rust. The design that can:

- `Box<'a, T>` is `std::boxed::Box<T>` plus `PhantomData<&'a ()>` (covariant, same size); Deref, DerefMut,
  `new_in`, `unbox`, `from_std`/`into_std`. The tree owns its nodes; dropping the `Program` frees them.
- `Vec<'a, T>` is `std::vec::Vec<T>` plus `PhantomData`, derefs to `std::vec::Vec<T>` (every method the parser
  calls exists there), `into_boxed_slice` gives `Box<'a, [T]>`.
- `HashMap`/`HashSet<'a, ..>` wrap `hashbrown::HashMap`/`HashSet` on the global allocator (hashbrown is std's
  table) with `default-features = false`, so `allocator-api2` is no longer needed by oxc.
- Strings the parser creates (`alloc_str`, `StringBuilder::into_str`, `alloc_concat_strs_array`, `FromIn<String>`
  for `&str`) go into a string arena: an array of 40 chunks, chunk `k` a boxed slice of `2^(k+6)`
  `OnceCell<Box<str>>` slots created once with `OnceCell::get_or_init`, and a `Cell<usize>` next-slot index.
  `OnceCell::get` returns `&str` tied to `&self`; filling other slots or chunks never moves a filled one, so
  `&'a str` from `&'a Allocator` is sound without `unsafe`. O(1) per string, one heap allocation per string,
  empty strings are `""`. `reset(&mut self)` replaces the arena.
- Source slices stay plain borrows of the source text.
- `Address` stays (a `usize` from a pointer cast is safe); `Address::from_ptr` becomes a safe fn; a `Box`'s
  address is the address of its heap value, which does not move when the `Box` moves.
- `CloneIn` for slices and vectors collects through iterators instead of writing through raw pointers;
  `TakeIn`, `Dummy`, `FromIn`/`IntoIn`, `AllocatorAccessor` keep their signatures.
- Removed: `Allocator::alloc(&self, T) -> &mut T`, `alloc_slice_copy`, `alloc_layout`, `into_bump_slice*`,
  `Box::dangling`, `Box::as_non_null`/`into_non_null`, `StringBuilder::push_*_unchecked`, the pool, fixed-size and
  raw-parts features. Users found so far:
  - nova: `parse_program` moves the program into the arena (`allocator.alloc(program)`) so that `AstNodes<'p>`
    can borrow it. Instead keep the `Program` in a local of `parse_source`/`parse_source_with_gc` and build the
    semantic data from `&program` (Program is covariant, so `&'q Program<'p>` is a `&'q Program<'q>`). This is a
    nova-only change that can land before the allocator.
  - `oxc_ast/src/ast/macros.rs`: `discriminant!` builds an enum value around `Box::dangling()` and reads its tag
    through a pointer; `shared_enum_variants!` converts `&Expression` to `&MemberExpression` (and the like) by
    transmute after a tag check. 799 of the first compile errors with the new allocator come from this one
    macro. Safe form: the tag constants as `const` items written out by the generator, and the conversions as
    `match` arms that move the `Box` (`into_*`) or return a borrowed view enum (`as_*`); the `as_*` callers in the
    parser and semantic change from `&MemberExpression` to the view.
  - `oxc_parser/src/lexer/unicode.rs`: two `push_*_unchecked` on the string builder; use `push_str` / `push`.
  - `oxc_ast_visit/src/generated/visit.rs` and `oxc_regular_expression/src/ast_impl/visit.rs`:
    `fn alloc<T>(&self, t: &T) -> &'a T` is a lifetime transmute ("honestly, I'm not really sure if this is
    safe"), which the semantic builder calls ~60 times to keep `AstKind<'a>` for nodes it visits. With the
    tree owned by `Program`, the visitor gets `&'a T` from `&'a Program<'a>` all the way down if `Visit` takes
    `it: &'a T` (oxc's `Visit<'a>` methods take `&T` today). Change the generator's signature for the visitor
    the semantic builder uses; nova's own walkers already hold `&'p` references.
- Status: landed in phase A (see above). `Box` (`#[repr(transparent)]`) and `Vec` wrap std's types
  with a `PhantomData<&'a ()>`; `from_std`/`into_std` convert. The `HashMap`/`HashSet` wrappers keep
  `into_keys`/`into_values`, `Deref` to hashbrown's table and the `*_in` constructors.
- Risks:
  - Drop now recurses through the tree (the arena never dropped). Depth is bounded by nova's nesting
    pre-scan (`source_nesting.rs`, 3000, which counts operator chains too), and drop frames are smaller than the
    parser's and the visitor's, so whatever parses and is analysed can be dropped. A partial tree dropped while
    the parser unwinds after an error is bounded by the same pre-scan, which runs before every parse.
  - One `malloc` per node and per string instead of a bump: slower parse and drop, more small allocations.
    Bundles are compiled once per process (`CompiledScript`), so only widget scripts pay it per engine.
  - Memory for the tree is no longer one block that `Allocator` frees; leaks are impossible without `unsafe`
    or `Box::leak`, which nothing may use.
  - `Vec` sizes: std's `Vec` (24 bytes) replaces oxc's (pointer, u32 length, u32 capacity, allocator: 24 bytes);
    `Box` stays 8 bytes. oxc's layout assertions are behind the `fixed_size` feature, which nova does not use.

## Area B: lexer and source reader (`oxc_parser`, 170)

Files: `lexer/source.rs` 58 (a raw-pointer cursor over the source bytes, `SourcePosition`), `lexer/template.rs` 33,
`lexer/comment.rs` 13, `lexer/identifier.rs` 10, `lexer/search.rs` 8 (byte-search tables and the
`byte_search!` macro), `lexer/mod.rs` 8, `lexer/byte_handlers.rs` 7, `lexer/token.rs` 7, `lexer/unicode.rs` 5,
`modifiers.rs` 5, `lexer/string.rs` 4, `lexer/jsx.rs` 3, `lexer/trivia_builder.rs` 3, `lib.rs` 3, `cursor.rs` 1,
`lexer/kind.rs` 1, `lexer/number.rs` 1; plus `memchr` (comments, JSX text, trivia; 333 in memchr).

Approach: `Source` becomes a `&'a [u8]` plus a `usize` position; `SourcePosition` a `usize`; every read is
`get(pos)` (a `None` is end of file, which the byte handlers already treat as a token). `str` slices come from
`source_text.get(start..end)`; the lexer only stops on UTF-8 boundaries, and `get` turns a mistake into a
panic, not UB. Byte search: a plain loop over `bytes[pos..].iter().position(..)` with the same 256-entry tables;
memchr calls become `position`/`windows` (nova already has a safe Two-Way search in `string/search.rs` if a
long needle is ever needed). `token.rs` packs a token into a `u128` with transmutes of `Kind`: use
`Kind::from_u8` via a `match` table generated with the enum. `modifiers.rs` transmutes bitflags; use
`from_bits_retain`.

Risks: the lexer is the hottest loop of the parser (expect 10-30% slower lexing from bounds checks); a lexer
change that moves a position by one byte changes tokens, so test262 parser tests and the widget hashes are the
check. Keep the work in small steps, one file at a time.

Status (2026-11-15, branch safe-oxc-b-lexer): done, oxc_parser 168 -> 0 with `#![forbid(unsafe_code)]`; vendored
oxc 407 -> 239, widget build 2368 -> 2200 (1417 without std's hashbrown). `Source` is the source `&str` plus a
`usize` offset; `SourcePosition` is the source bytes plus an offset, so `pos.read()`/`add`/`slice` keep their
call sites; every `&str` comes from `str` indexing, which panics rather than split a character. `byte_search!`
reads checked 32-byte batches; the memchr calls are slice searches (memchr stays only through oxc_semantic);
`Token` is a 16-byte struct of fields instead of a `u128` with transmutes; `Modifiers` keeps plain `u32` offsets.
Checked against the old lexer (tokens, AST `Debug` and errors, as script, module and JSX, with and without token
collection, identical; no panics in either): the bundle, 415 widget scripts, 53,910 test262 files, every 5th-byte
prefix of 2,571 of those scripts, every 1499th-byte prefix of the bundle, and 1.2M generated odd inputs (lone
surrogate escapes, every line terminator, BOMs, 100 KB identifiers and numbers, unterminated strings, templates,
comments and JSX at end of input). Parse, best of 4 interleaved runs: bundle parse only 5.52 -> 5.78 ms (+4.7%),
parse plus semantic 8.55 -> 8.76 ms (+2.5%), the widget scripts +0-4%.

## Area C: data structures and indexes

Files: `oxc_data_structures` 151 (`stack/` 96: `Stack`, `NonEmptyStack`, `SparseStack` on raw pointers;
`slice_iter.rs` 23; `code_buffer.rs` 20, unused by nova's build if no crate enables `code_buffer`;
`assert_unchecked.rs` 7; `inline_string.rs` 5), `oxc_index` 29 (`idxslice.rs` 9, `lib.rs` 5, `macros.rs` 4,
`rayon_impl.rs` 11 not compiled), `oxc_semantic/src/multi_index_vec.rs` 13, `compact_str` 159 + `castaway` 53 +
`static_assertions` 1 (through `oxc_span`/`oxc_str`: `CompactStr`), `self_cell` 34 (`oxc_semantic/src/scoping.rs`:
`Scoping` owns an `Allocator` beside tables that borrow it), `nonmax` 10.

Approach: stacks become `std::vec::Vec` with the same API (`last`/`last_mut` with `expect` for the non-empty
one); `assert_unchecked!` becomes `debug_assert!`; `IndexVec`/`IndexSlice` keep their API over `Vec<T>` and
`[T]` with checked indexing, and the slice newtype cast (`&[T]` to `&IndexSlice<I, [T]>`, a transmute of a
`repr(transparent)` type) becomes a wrapper struct holding the slice; `CompactStr` becomes `Box<str>` (it is
used for diagnostics and owned names, not hot paths); `NonMaxU32` becomes a `NonZeroU32` storing `value + 1`
(same niche); `Scoping` stops borrowing an arena: with area A its vectors and maps own their data, and its
names can be `Box<str>` or live in a string arena it owns behind an index instead of a `&str`, which removes
self_cell.

Risks: `ScopeId`/`SymbolId` lose nothing (same niche), but anything that relied on `IndexSlice` being
`repr(transparent)` over `[T]` needs the wrapper.

## Area D: diagnostics (`oxc-miette` 89, `percent-encoding` 4)

Files: `oxc_diagnostics/src/{lib,reporter,service}.rs`, `oxc_span/src/span.rs` (`From<Span> for SourceSpan` /
`LabeledSpan`), every `OxcDiagnostic::error(..).with_label(..)` in the parser and semantic (no change needed if
the builder API stays), `nova_vm` (`source_code.rs` reads `error.message` and `error.labels`), `nova_cli`
(`exit_with_parse_errors` uses miette's graphical handler).

Approach: `OxcDiagnostic` becomes a plain struct (message, labels as `(Span, Option<Cow<str>>)`, help, severity,
code) with the same builder methods and `Display`; `Error` an alias for it or a boxed std error. The terminal
report in `nova_cli` prints `file:line:col: message` plus the source line, or keeps miette under `nova_cli` only
(the CLI is not the widget engine). `percent-encoding` is only used for doc URLs in `service.rs`; drop it.

Risks: error messages are part of observable behaviour (SyntaxError messages in widgets and test262 negative
tests compare only the type, but widget output can print messages). Keep `message` byte-identical.

Status (2026-11-15, done): `OxcDiagnostic` keeps its fields, builder methods and `Display`; `LabeledSpan`,
`SourceSpan` and `Severity` are plain types in oxc_diagnostics (which forbids `unsafe_code`), and oxc_span converts
a `Span` into them. The reporter and `DiagnosticService` are removed (no user in nova). `nova_cli` prints each
message and, per label, `file:line:column`, the source line and an underline with the label text; its transmute
to a `'static` source went with miette. Every diagnostic on 26977 test262 and bundle files (12345 errors, 263
distinct messages: message, help, label offsets, lengths and texts) is byte-identical before and after.
oxc-miette, percent-encoding, thiserror, unicode-width and unicode-segmentation left the build.

## Area E: numbers (`ryu` 27, `itoa` 16, `dragonbox_ecma` 20)

`ryu`/`itoa` come only with compact_str (gone with area C). `dragonbox_ecma` is behind `oxc_syntax`'s
`to_js_string` feature, which `oxc_ecmascript` turns on for constant folding (`ToJsString` on numbers); nova does
not need oxc's folding to print numbers. Approach: use nova's `number/text.rs` logic (std `{:e}`/`{:.N}` with the
ECMAScript shortest-digits rules), or turn the feature off and cut the callers in oxc_ecmascript that nova
never calls. Risk: none for nova's output if nova never formats through oxc; check with a grep for
`to_js_string` in nova and the gate.

Status (2026-11-15): dragonbox_ecma is gone; `ToJsString for f64` in oxc_syntax is nova's `f64_to_js_string`
(`oxc_syntax/src/number/to_js_string.rs`), identical to dragonbox_ecma on 26.2 million values including every
tie candidate. `ryu` and `itoa` stay until area C replaces `CompactStr` (compact_str is their only user; oxc
does not call them).

## Area F: small dependencies

- `memchr` (333): only `oxc_parser` (area B) and `oxc_semantic/src/checker/javascript.rs`; replace with slice
  search. Then it leaves the widget build entirely.
- `num-bigint` 0.4.8 (8) via oxc_parser/oxc_ecmascript for BigInt literals: move them to nova's 0.5.1 (also 8, two
  `from_utf8_unchecked` in `to_str_radix` and x86 intrinsics) or parse BigInt literals into nova's own type;
  then fork 0.5.1 with the two `from_utf8_unchecked` replaced by `from_utf8(..).expect(..)`.
- `unicode-normalization` (5): fork without `char::from_u32_unchecked` (use `char::from_u32(..).expect(..)`).
- `bitflags` (2), `unicode-id-start` (2), `cow-utils` (7), `itertools` (10) + `either` (2), `phf` + `siphasher`
  (12): `bitflags`' two are in its generated `from_bits_unchecked`-style API (a fork or a hand-written flags type
  per use); `unicode-id-start` is two `get_unchecked` on its tables (vendor and use checked indexing);
  `cow-utils` is a handful of calls (`cow_replace`, `cow_to_ascii_lowercase`) that std `Cow` code replaces;
  itertools is a few adapters in oxc_semantic (std iterators); phf maps (keyword tables) become `match`
  statements or sorted slices.

Status (2026-11-15):
- memchr: done. The lexer's three uses go through `oxc_parser/src/byte_search.rs` (safe `memchr`,
  `memchr_iter`, `memmem::Finder` with the same names), the semantic checker's through a filtered iterator.
  Comments (215384 on test262 and the bundles, with their kind and legal flag) are identical before and after.
  memchr stays in the full workspace only through combine (temporal_rs), noyalib and serde_json.
- bitflags, cow-utils, unicode-id-start: the root `Cargo.toml` patches them to copies in `oxc/vendor/` with
  `#![forbid(unsafe_code)]`: checked indexing in unicode-id-start (same answers for every scalar value) and
  cow-utils, bitflags without its `bytemuck` impls. Sources of the oxc crates are unchanged.
- num-bigint 0.4, itertools/either and phf/siphasher moved to area G; unicode-normalization, num-bigint 0.5 and
  num-traits are nova_vm's own dependencies (not this track).

## Area G: oxc_ast, oxc_ast_visit, oxc_semantic, oxc_ecmascript, oxc_syntax, oxc_str, oxc_estree, oxc_regular_expression

- `oxc_ast` (6): the `discriminant!`/`shared_enum_variants!` transmutes (see area A), `ast_kind.rs` (address of a
  node), `ast_impl/literal.rs` (a `from_utf8_unchecked`), `generated/ast_builder.rs` (one `Box::dangling` style
  helper). Owned by whoever does area A's oxc_ast part; the generated files must be regenerated or edited
  consistently.
- `oxc_ast_visit` (20): the `alloc` transmute (area A) and `utf8_to_utf16/` (19, span conversion for ESTree; not
  used by nova, drop the module).
- `oxc_semantic` (18): `multi_index_vec.rs` 13 (area C), `ast_types_bitset.rs` 2, `scoping.rs` 2 (self_cell),
  `checker/javascript.rs` 1.
- `oxc_ecmascript` (10): `to_int_32.rs` 5 (bit tricks on f64, std `to_bits` and arithmetic), `call_expr.rs` 2,
  `url_encoding/enc.rs` 3 (`from_utf8_unchecked`).
- `oxc_syntax` (7): `identifier.rs` 2, `line_terminator.rs` 5 (`from_utf8_unchecked` on constant byte strings:
  use `&str` constants).
- `oxc_str` (5): `ident.rs` (an `Ident` with a packed length/hash; the transmutes become field reads).
- `oxc_estree` (17): only the serializer, which nova does not compile (`serialize` off); remove the dependency
  from `oxc_ast`, `oxc_span`, `oxc_str`, `oxc_syntax` and `oxc_allocator` together with their `ESTree` impls.
- `oxc_regular_expression` (1): its own `alloc` transmute in the visitor (area A's pattern).

## Measurements (phase 0)

Parse plus semantic check (`Parser::parse` + `SemanticBuilder::with_check_syntax_error(true)`), release, best of
N on a loaded machine, vendored oxc before the allocator change (phase-0 scratch `parse-before.txt`):

| input | bytes | best |
|---|---:|---:|
| sandbox-bundle.js | 597800 | 7.6 ms |
| 07-shop-06-shop-bare (scripts) | 10534 | 0.077 ms |
| 08-analytics-10-bot-bare | 8945 | 0.099 ms |
| 12-motion-lab-02-mo-bare | 8732 | 0.081 ms |
| hostile/text-chaos | 1741 | 0.017 ms |

Phase A's table above has the numbers with the safe allocator.

## Order and parallel work

1. A: allocator, then the oxc_ast macro rewrite and the visitor signature, nova's `parse_program` (can land first).
2. In parallel with A once it compiles: B (lexer), C (data structures), D (diagnostics), F (memchr and the small
   crates). E and the oxc_estree removal are small and can go with C.
3. G's leftovers per crate. Each step: the gate (`safe/tools/gate.sh`), the widget hashes, and
   `dep_unsafe_report.py` going down; `#![forbid(unsafe_code)]` on each crate as it reaches 0.
