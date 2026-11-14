# Trusted computing base of nova_vm

`nova_vm`, `small_string`, `wtf8` and `nova_vm_derive` forbid `unsafe_code`. What a script can still reach that
is not checked by the compiler is Rust's std and the `unsafe` inside the crates nova_vm depends on. This file lists
those crates, how much `unsafe` each has, whether input from a script reaches it, and what was done about it.

## Method

- Crates: `cargo tree -p nova_vm -e normal,no-proc-macro`, the code linked into the process. Proc-macro crates and
  their dependencies (`nova_vm_derive`, `oxc_ast_macros`, `syn`, `quote`, ...) and build scripts run in the compiler,
  not in the process, and are not counted.
- Count: occurrences of the `unsafe` keyword outside comments in each crate's `.rs` files, without `tests/`,
  `benches/` and `examples/` (`unsafe` blocks, `unsafe fn`, `unsafe impl` and `unsafe trait` each count once).
  Code behind `cfg` for other targets (x86 SIMD, Windows, wasm) is counted too, so the numbers are an upper bound.
- Script input: whether bytes or values a script chooses reach the crate's code (source text, strings, numbers,
  keys, sizes).

## Summary

| build | crates | with `unsafe` | `unsafe` |
|---|---:|---:|---:|
| before (`safe` dca5a92c, default features) | 122 | 88 | 6561 |
| after, default features | 96 | 64 | 3953 |
| after, `--no-default-features --features widget` | 81 | 58 | 3518 |

Removed from the tree: sonic-rs and its 7 dependencies (794), ryu-js, lexical (8 crates) and fast-float (187),
ahash with zerocopy, once_cell, getrandom 0.3 and libc (1436), rand with rand_core, chacha20 and getrandom 0.4 (191).
memchr is no longer used by nova_vm, but stays in the tree through oxc and regex. The `widget` feature set leaves
out Temporal (temporal_rs and 14 crates: icu_calendar, iana-time-zone, core-foundation-sys, bytes, combine, libm,
tzif, ...), SharedArrayBuffer and Atomics.

## What replaced them

| was | now | script input |
|---|---|---|
| sonic-rs (JSON.parse) | `builtins/structured_data/json_object/json_parser.rs`: ECMA-404 grammar on the WTF-8 bytes, explicit stack (nesting over 5000 is a RangeError), std `str::parse::<f64>` | the JSON text |
| ryu-js (toString, toFixed, JSON numbers, numeric keys) | `types/language/number/text.rs` on std `{:e}` (shortest digits, ties to even fixed with exact u128 arithmetic) and `{:.N}` | numbers |
| lexical (toString(radix)) | `number/text.rs`: integer digits, V8's algorithm for fractions | numbers |
| fast-float (Number(string), parseFloat) | `number/text.rs` literal scanner + std `str::parse::<f64>` | strings |
| memchr (indexOf, lastIndexOf, includes, split) | `types/language/string/search.rs`: Two-Way, O(n + m), u8 and u16 | both strings |
| ahash (heap strings, property keys, Map/Set keys, side tables) | `hashing.rs`: std SipHash-1-3 with random keys | keys |
| rand (Math.random) | xorshift128+ per thread, seeded from std's `RandomState` (`math_object.rs`) | none |

Bugs found in what was replaced:
- ryu-js `format_to_fixed` writes into an uninitialized stack buffer and returns bytes it did not write for small
  numbers with many digits: `(4.2025059341068935e-46).toFixed(65)` gave `"0.2121513485908508300781250000..."`,
  stack memory read into a script's string.
- `AHasher::default()` has fixed keys: Map, Set, `Object.groupBy`/`Map.groupBy` and every property key in shapes
  and caches hashed with keys a script could know, so colliding keys could be precomputed.
- JSON.parse recursed per nesting level (sonic-rs's tree and nova's conversion): a deep text overflowed the stack.
- The UTF-16 search path (needles that start with a trail or end with a lead surrogate) was O(n * m).

Performance (release builds, best of several runs on a machine shared with five other builds):
- JSON.parse of a 5.7 MB document (18000 records): 65.4 ms with sonic-rs, 61.8 ms now.
- 200000 Map.set + Map.get: integer keys 169 -> 272 ms, string keys 187 -> 245 ms (SipHash instead of aHash);
  object literals and string interning unchanged within noise.
- JSON.stringify of 50000 floats 24.7 -> 33.8 ms (std formatting instead of ryu-js).
- Search of 20 MB of text for a missing 8-byte needle: 5 ms (memchr 3 ms); worst case ("a" * 2e7 against
  "a" * 1e4 + "b") 49 ms, linear.

## What stays, and why

- **std**: the base everything stands on.
- **hashbrown** (454 + 329 in oxc's copy): std's HashMap is hashbrown; using it directly adds no new kind of code.
  allocator-api2 and foldhash come with it.
- **oxc** (oxc_parser, oxc_allocator, oxc_ast, oxc_semantic, oxc_ecmascript, oxc_regular_expression and their
  dependencies: compact_str, castaway, smallvec, num-bigint 0.4, the icu/zerovec family behind oxc-miette's
  diagnostics): it parses the script source, which a widget controls entirely. 1722 `unsafe` in the crates only
  oxc uses, plus its own hashbrown 0.16 (329) and memchr (333, shared with regex); replacing the parser is out of
  scope for this track. The largest parts are the bump allocator
  (oxc_allocator, 273), the lexer (oxc_parser, 170) and compact_str (159).
- **regex** (regex, regex-automata, aho-corasick, memchr): the regexp track replaces it with an in-tree engine.
- **soavec** (151): the heap's struct-of-arrays storage; script sizes reach it. A safe replacement belongs to the
  heap track (c-heap).
- **num-bigint 0.5** (8): BigInt arithmetic on script values. The `unsafe` is `from_utf8_unchecked` on digits it
  wrote itself in `to_str_radix` and x86 carry intrinsics that are not compiled on arm64.
- **unicode-normalization** (5): `String.prototype.normalize`; `char::from_u32_unchecked` on Hangul syllable
  arithmetic. Small enough to fork without `unsafe` later.
- **Temporal** (temporal_rs and its dependencies): off in the widget build.

## Crates before (default features)

| crate | version | unsafe | reachable from JS input | decision |
|---|---|---:|---|---|
| libc | 0.2.191 | 800 | getrandom (ahash, rand); iana-time-zone (temporal) | gone from the widget build with ahash, rand and temporal |
| zerocopy | 0.8.63 | 490 | ahash | gone with ahash |
| hashbrown | 0.17.1 | 454 | every HashTable (strings, shapes, Map/Set, caches); keys and sizes from scripts | keep: it is std's own HashMap, already in the trusted base |
| memchr | 2.8.3 | 333 | indexOf/lastIndexOf/includes/split (both strings), script source (oxc_parser, oxc_semantic), regex | our use replaced by a safe Two-Way search (string/search.rs); still pulled by oxc_parser, oxc_semantic, regex, temporal_rs |
| hashbrown | 0.16.1 | 329 | every HashTable (strings, shapes, Map/Set, caches); keys and sizes from scripts | keep: it is std's own HashMap, already in the trusted base |
| oxc_allocator | 0.124.0 | 273 | script source text (parser, AST, scopes, diagnostics) | keep: the parser; too big to replace here |
| zerovec | 0.11.9 | 236 | script source text (parser, AST, scopes, diagnostics) | keep: the parser; too big to replace here |
| aho-corasick | 1.1.5 | 227 | RegExp | regexp track |
| sonic-rs | 0.5.10 | 227 | JSON.parse text | replaced: in-tree JSON parser (json_object/json_parser.rs) |
| allocator-api2 | 0.2.21 | 214 | hashbrown, oxc_allocator | keep (hashbrown, oxc) |
| iana-time-zone | 0.1.65 | 212 | Temporal (dates, time zones, calendars) | off in the widget build |
| bumpalo | 3.20.4 | 186 | JSON.parse (sonic-rs arena) | gone with sonic-rs |
| oxc_parser | 0.124.0 | 170 | script source text (parser, AST, scopes, diagnostics) | keep: the parser; too big to replace here |
| compact_str | 0.9.1 | 159 | script source text (parser, AST, scopes, diagnostics) | keep: the parser; too big to replace here |
| oxc_data_structures | 0.124.0 | 151 | script source text (parser, AST, scopes, diagnostics) | keep: the parser; too big to replace here |
| soavec | 0.2.0 | 151 | the heap's struct-of-arrays vectors (sizes from scripts) | keep for now: c-heap's storage; a safe rewrite belongs to that track |
| bytes | 1.12.1 | 138 | Temporal (dates, time zones, calendars) | off in the widget build |
| simdutf8 | 0.1.5 | 135 | JSON.parse text (sonic-rs) | gone with sonic-rs |
| getrandom | 0.4.3 | 109 | seeds (ahash, rand) | gone with ahash and rand |
| getrandom | 0.3.4 | 92 | seeds (ahash, rand) | gone with ahash and rand |
| sonic-simd | 0.1.4 | 91 | JSON.parse text (sonic-rs) | gone with sonic-rs |
| yoke | 0.8.4 | 90 | script source text (parser, AST, scopes, diagnostics) | keep: the parser; too big to replace here |
| oxc-miette | 2.7.1 | 89 | script source text (parser, AST, scopes, diagnostics) | keep: the parser; too big to replace here |
| smallvec | 1.16.5 | 80 | script source text (parser, AST, scopes, diagnostics) | keep: the parser; too big to replace here |
| zmij | 1.0.23 | 70 | number printing in sonic-rs | gone with sonic-rs |
| chacha20 | 0.10.2 | 66 | Math.random | gone with rand |
| faststr | 0.2.34 | 57 | JSON.parse strings (sonic-rs) | gone with sonic-rs |
| libm | 0.2.16 | 56 | Temporal (dates, time zones, calendars) | off in the widget build |
| regex-automata | 0.4.18 | 55 | RegExp | regexp track |
| castaway | 0.2.4 | 53 | script source text (parser, AST, scopes, diagnostics) | keep: the parser; too big to replace here |
| lexical-util | 1.0.7 | 50 | toString(radix) | gone with lexical |
| lexical-parse-float | 1.0.6 | 48 | none (parse side unused) | gone with lexical |
| once_cell | 1.21.4 | 47 | ahash | gone with ahash |
| ryu-js | 1.0.3 | 39 | Number toString/toFixed, JSON.stringify, numeric keys | replaced: std `{:e}` / `{:.N}` + number/text.rs (it leaked stack memory through toFixed) |
| self_cell | 1.3.0 | 34 | script source text (parser, AST, scopes, diagnostics) | keep: the parser; too big to replace here |
| utf_types | 1.0.0 | 31 | script source text (parser, AST, scopes, diagnostics) | keep: the parser; too big to replace here |
| oxc_index | 4.1.0 | 29 | script source text (parser, AST, scopes, diagnostics) | keep: the parser; too big to replace here |
| ryu | 1.0.23 | 27 | script source text (parser, AST, scopes, diagnostics) | keep: the parser; too big to replace here |
| stable_deref_trait | 1.2.1 | 25 | script source text (parser, AST, scopes, diagnostics) | keep: the parser; too big to replace here |
| sonic-number | 0.1.3 | 24 | JSON.parse numbers (sonic-rs) | gone with sonic-rs |
| lexical-write-integer | 1.0.6 | 23 | toString(radix) | gone with lexical |
| tinystr | 0.8.5 | 22 | script source text (parser, AST, scopes, diagnostics) | keep: the parser; too big to replace here |
| dragonbox_ecma | 0.1.12 | 20 | script source text (parser, AST, scopes, diagnostics) | keep: the parser; too big to replace here |
| oxc_ast_visit | 0.124.0 | 20 | script source text (parser, AST, scopes, diagnostics) | keep: the parser; too big to replace here |
| icu_provider | 2.4.0 | 19 | script source text (parser, AST, scopes, diagnostics) | keep: the parser; too big to replace here |
| oxc_semantic | 0.124.0 | 18 | script source text (parser, AST, scopes, diagnostics) | keep: the parser; too big to replace here |
| oxc_estree | 0.124.0 | 17 | script source text (parser, AST, scopes, diagnostics) | keep: the parser; too big to replace here |
| itoa | 1.0.19 | 16 | script source text (parser, AST, scopes, diagnostics) | keep: the parser; too big to replace here |
| rand | 0.10.3 | 16 | Math.random | replaced: xorshift128+ seeded from std RandomState |
| utf8_iter | 1.1.0 | 16 | script source text (parser, AST, scopes, diagnostics) | keep: the parser; too big to replace here |
| combine | 4.6.9 | 15 | Temporal (dates, time zones, calendars) | off in the widget build |
| icu_collections | 2.4.0 | 15 | script source text (parser, AST, scopes, diagnostics) | keep: the parser; too big to replace here |
| core-foundation-sys | 0.8.7 | 13 | Temporal (dates, time zones, calendars) | off in the widget build |
| siphasher | 1.0.4 | 12 | script source text (parser, AST, scopes, diagnostics) | keep: the parser; too big to replace here |
| fast-float | 0.2.0 | 10 | Number(string), parseFloat | replaced: std str::parse::<f64> + our literal scanner |
| foldhash | 0.2.0 | 10 | hashbrown default hasher (unused by nova_vm) | keep (hashbrown feature) |
| itertools | 0.14.0 | 10 | script source text (parser, AST, scopes, diagnostics) | keep: the parser; too big to replace here |
| lexical-write-float | 1.0.6 | 10 | toString(radix) | gone with lexical |
| nonmax | 0.5.5 | 10 | script source text (parser, AST, scopes, diagnostics) | keep: the parser; too big to replace here |
| oxc_ecmascript | 0.124.0 | 10 | script source text (parser, AST, scopes, diagnostics) | keep: the parser; too big to replace here |
| potential_utf | 0.1.6 | 10 | script source text (parser, AST, scopes, diagnostics) | keep: the parser; too big to replace here |
| num-bigint | 0.4.8 | 8 | BigInt literals in script source (oxc_parser, oxc_ecmascript); Temporal | keep with oxc |
| num-bigint | 0.5.1 | 8 | BigInt arithmetic and parsing | keep: 2 from_utf8_unchecked in to_str_radix on digits it wrote, and x86 carry intrinsics not compiled on arm64 |
| ahash | 0.8.12 | 7 | every property key, Map/Set key and heap string hash | replaced: std SipHash-1-3, random keys (hashing.rs) |
| cow-utils | 0.1.3 | 7 | script source text (parser, AST, scopes, diagnostics) | keep: the parser; too big to replace here |
| oxc_syntax | 0.124.0 | 7 | script source text (parser, AST, scopes, diagnostics) | keep: the parser; too big to replace here |
| zerotrie | 0.2.5 | 7 | script source text (parser, AST, scopes, diagnostics) | keep: the parser; too big to replace here |
| icu_locale_core | 2.4.0 | 6 | script source text (parser, AST, scopes, diagnostics) | keep: the parser; too big to replace here |
| oxc_ast | 0.124.0 | 6 | script source text (parser, AST, scopes, diagnostics) | keep: the parser; too big to replace here |
| lexical-parse-integer | 1.0.6 | 5 | none (parse side unused) | gone with lexical |
| oxc_str | 0.124.0 | 5 | script source text (parser, AST, scopes, diagnostics) | keep: the parser; too big to replace here |
| unicode-normalization | 0.1.26 | 5 | String.prototype.normalize | keep for now: 5 unsafe, char::from_u32_unchecked on Hangul arithmetic; small enough to fork later |
| percent-encoding | 2.3.2 | 4 | script source text (parser, AST, scopes, diagnostics) | keep: the parser; too big to replace here |
| ref-cast | 1.0.27 | 4 | sonic-rs | gone with sonic-rs |
| writeable | 0.6.4 | 3 | script source text (parser, AST, scopes, diagnostics) | keep: the parser; too big to replace here |
| bitflags | 2.13.2 | 2 | script source text (parser, AST, scopes, diagnostics) | keep: the parser; too big to replace here |
| either | 1.19.0 | 2 | script source text (parser, AST, scopes, diagnostics) | keep: the parser; too big to replace here |
| icu_segmenter | 2.4.0 | 2 | script source text (parser, AST, scopes, diagnostics) | keep: the parser; too big to replace here |
| lexical | 7.0.5 | 2 | Number/BigInt toString(radix) | replaced: number/text.rs |
| serde | 1.0.229 | 2 | script source text (parser, AST, scopes, diagnostics) | keep: the parser; too big to replace here |
| serde_core | 1.0.229 | 2 | script source text (parser, AST, scopes, diagnostics) | keep: the parser; too big to replace here |
| unicode-id-start | 1.5.0 | 2 | script source text (parser, AST, scopes, diagnostics) | keep: the parser; too big to replace here |
| icu_calendar | 2.4.0 | 1 | Temporal (dates, time zones, calendars) | off in the widget build |
| icu_locale_fallback | 2.4.0 | 1 | script source text (parser, AST, scopes, diagnostics) | keep: the parser; too big to replace here |
| num-traits | 0.2.19 | 1 | BigInt, numbers | keep |
| oxc_regular_expression | 0.124.0 | 1 | script source text (parser, AST, scopes, diagnostics) | keep: the parser; too big to replace here |
| regex | 1.13.1 | 1 | RegExp | regexp track (d-strings) replaces it |
| static_assertions | 1.1.0 | 1 | script source text (parser, AST, scopes, diagnostics) | keep: the parser; too big to replace here |
| calendrical_calculations | 0.2.4 | 0 | Temporal (dates, time zones, calendars) | off in the widget build |
| cfg-if | 1.0.5 | 0 | script source text (parser, AST, scopes, diagnostics) | keep: the parser; too big to replace here |
| core_maths | 0.1.1 | 0 | Temporal (dates, time zones, calendars) | off in the widget build |
| equivalent | 1.0.3 | 0 | hashbrown | keep |
| icu_calendar_data | 2.4.0 | 0 | Temporal (dates, time zones, calendars) | off in the widget build |
| icu_locale_fallback_data | 2.4.0 | 0 | script source text (parser, AST, scopes, diagnostics) | keep: the parser; too big to replace here |
| icu_segmenter_data | 2.4.0 | 0 | script source text (parser, AST, scopes, diagnostics) | keep: the parser; too big to replace here |
| ixdtf | 0.6.7 | 0 | Temporal (dates, time zones, calendars) | off in the widget build |
| jiff-tzdb | 0.1.9 | 0 | Temporal (dates, time zones, calendars) | off in the widget build |
| lexical-core | 1.0.6 | 0 | toString(radix) | gone with lexical |
| litemap | 0.8.3 | 0 | script source text (parser, AST, scopes, diagnostics) | keep: the parser; too big to replace here |
| num-integer | 0.1.47 | 0 | BigInt | keep |
| owo-colors | 4.4.0 | 0 | script source text (parser, AST, scopes, diagnostics) | keep: the parser; too big to replace here |
| oxc_diagnostics | 0.124.0 | 0 | script source text (parser, AST, scopes, diagnostics) | keep: the parser; too big to replace here |
| oxc_span | 0.124.0 | 0 | script source text (parser, AST, scopes, diagnostics) | keep: the parser; too big to replace here |
| phf | 0.13.1 | 0 | script source text (parser, AST, scopes, diagnostics) | keep: the parser; too big to replace here |
| phf_shared | 0.13.1 | 0 | script source text (parser, AST, scopes, diagnostics) | keep: the parser; too big to replace here |
| rand_core | 0.10.1 | 0 | Math.random | gone with rand |
| regex-syntax | 0.8.11 | 0 | RegExp | regexp track |
| rustc-hash | 2.1.3 | 0 | script source text (parser, AST, scopes, diagnostics) | keep: the parser; too big to replace here |
| small_string | 1.0.0 | 0 | strings | ours, `#![forbid(unsafe_code)]` |
| smawk | 0.3.3 | 0 | script source text (parser, AST, scopes, diagnostics) | keep: the parser; too big to replace here |
| temporal_rs | 0.2.6 | 0 | Temporal | off in the widget build (feature `widget`) |
| textwrap | 0.16.4 | 0 | script source text (parser, AST, scopes, diagnostics) | keep: the parser; too big to replace here |
| thiserror | 2.0.21 | 0 | script source text (parser, AST, scopes, diagnostics) | keep: the parser; too big to replace here |
| timezone_provider | 0.2.6 | 0 | Temporal (dates, time zones, calendars) | off in the widget build |
| tinyvec | 1.13.3 | 0 | unicode-normalization | keep (0 unsafe) |
| tzif | 0.4.1 | 0 | Temporal (dates, time zones, calendars) | off in the widget build |
| unicode-segmentation | 1.13.3 | 0 | script source text (parser, AST, scopes, diagnostics) | keep: the parser; too big to replace here |
| unicode-width | 0.2.3 | 0 | script source text (parser, AST, scopes, diagnostics) | keep: the parser; too big to replace here |
| web-time | 1.1.0 | 0 | Temporal (dates, time zones, calendars) | off in the widget build |
| wtf8 | 1.0.0 | 0 | strings | ours, `#![forbid(unsafe_code)]` |
| xsum | 0.1.6 | 0 | Math.sumPrecise | keep (0 unsafe) |
| zerofrom | 0.1.8 | 0 | script source text (parser, AST, scopes, diagnostics) | keep: the parser; too big to replace here |

122 crates, 88 with unsafe, 6561 unsafe keywords in all.

## Crates after (`--no-default-features --features widget`)

| crate | version | unsafe | reachable from JS input | decision |
|---|---|---:|---|---|
| hashbrown | 0.17.1 | 454 | every HashTable (strings, shapes, Map/Set, caches); keys and sizes from scripts | keep: it is std's own HashMap, already in the trusted base |
| memchr | 2.8.3 | 333 | indexOf/lastIndexOf/includes/split (both strings), script source (oxc_parser, oxc_semantic), regex | our use replaced by a safe Two-Way search (string/search.rs); still pulled by oxc_parser, oxc_semantic, regex, temporal_rs |
| hashbrown | 0.16.1 | 329 | every HashTable (strings, shapes, Map/Set, caches); keys and sizes from scripts | keep: it is std's own HashMap, already in the trusted base |
| oxc_allocator | 0.124.0 | 273 | script source text (parser, AST, scopes, diagnostics) | keep: the parser; too big to replace here |
| zerovec | 0.11.9 | 236 | script source text (parser, AST, scopes, diagnostics) | keep: the parser; too big to replace here |
| aho-corasick | 1.1.5 | 227 | RegExp | regexp track |
| allocator-api2 | 0.2.21 | 214 | hashbrown, oxc_allocator | keep (hashbrown, oxc) |
| oxc_parser | 0.124.0 | 170 | script source text (parser, AST, scopes, diagnostics) | keep: the parser; too big to replace here |
| compact_str | 0.9.1 | 159 | script source text (parser, AST, scopes, diagnostics) | keep: the parser; too big to replace here |
| oxc_data_structures | 0.124.0 | 151 | script source text (parser, AST, scopes, diagnostics) | keep: the parser; too big to replace here |
| soavec | 0.2.0 | 151 | the heap's struct-of-arrays vectors (sizes from scripts) | keep for now: c-heap's storage; a safe rewrite belongs to that track |
| yoke | 0.8.4 | 90 | script source text (parser, AST, scopes, diagnostics) | keep: the parser; too big to replace here |
| oxc-miette | 2.7.1 | 89 | script source text (parser, AST, scopes, diagnostics) | keep: the parser; too big to replace here |
| smallvec | 1.16.5 | 80 | script source text (parser, AST, scopes, diagnostics) | keep: the parser; too big to replace here |
| regex-automata | 0.4.18 | 55 | RegExp | regexp track |
| castaway | 0.2.4 | 53 | script source text (parser, AST, scopes, diagnostics) | keep: the parser; too big to replace here |
| self_cell | 1.3.0 | 34 | script source text (parser, AST, scopes, diagnostics) | keep: the parser; too big to replace here |
| utf_types | 1.0.0 | 31 | script source text (parser, AST, scopes, diagnostics) | keep: the parser; too big to replace here |
| oxc_index | 4.1.0 | 29 | script source text (parser, AST, scopes, diagnostics) | keep: the parser; too big to replace here |
| ryu | 1.0.23 | 27 | script source text (parser, AST, scopes, diagnostics) | keep: the parser; too big to replace here |
| stable_deref_trait | 1.2.1 | 25 | script source text (parser, AST, scopes, diagnostics) | keep: the parser; too big to replace here |
| tinystr | 0.8.5 | 22 | script source text (parser, AST, scopes, diagnostics) | keep: the parser; too big to replace here |
| dragonbox_ecma | 0.1.12 | 20 | script source text (parser, AST, scopes, diagnostics) | keep: the parser; too big to replace here |
| oxc_ast_visit | 0.124.0 | 20 | script source text (parser, AST, scopes, diagnostics) | keep: the parser; too big to replace here |
| icu_provider | 2.4.0 | 19 | script source text (parser, AST, scopes, diagnostics) | keep: the parser; too big to replace here |
| oxc_semantic | 0.124.0 | 18 | script source text (parser, AST, scopes, diagnostics) | keep: the parser; too big to replace here |
| oxc_estree | 0.124.0 | 17 | script source text (parser, AST, scopes, diagnostics) | keep: the parser; too big to replace here |
| itoa | 1.0.19 | 16 | script source text (parser, AST, scopes, diagnostics) | keep: the parser; too big to replace here |
| utf8_iter | 1.1.0 | 16 | script source text (parser, AST, scopes, diagnostics) | keep: the parser; too big to replace here |
| icu_collections | 2.4.0 | 15 | script source text (parser, AST, scopes, diagnostics) | keep: the parser; too big to replace here |
| siphasher | 1.0.4 | 12 | script source text (parser, AST, scopes, diagnostics) | keep: the parser; too big to replace here |
| foldhash | 0.2.0 | 10 | hashbrown default hasher (unused by nova_vm) | keep (hashbrown feature) |
| itertools | 0.14.0 | 10 | script source text (parser, AST, scopes, diagnostics) | keep: the parser; too big to replace here |
| nonmax | 0.5.5 | 10 | script source text (parser, AST, scopes, diagnostics) | keep: the parser; too big to replace here |
| oxc_ecmascript | 0.124.0 | 10 | script source text (parser, AST, scopes, diagnostics) | keep: the parser; too big to replace here |
| potential_utf | 0.1.6 | 10 | script source text (parser, AST, scopes, diagnostics) | keep: the parser; too big to replace here |
| num-bigint | 0.4.8 | 8 | BigInt literals in script source (oxc_parser, oxc_ecmascript); Temporal | keep with oxc |
| num-bigint | 0.5.1 | 8 | BigInt arithmetic and parsing | keep: 2 from_utf8_unchecked in to_str_radix on digits it wrote, and x86 carry intrinsics not compiled on arm64 |
| cow-utils | 0.1.3 | 7 | script source text (parser, AST, scopes, diagnostics) | keep: the parser; too big to replace here |
| oxc_syntax | 0.124.0 | 7 | script source text (parser, AST, scopes, diagnostics) | keep: the parser; too big to replace here |
| zerotrie | 0.2.5 | 7 | script source text (parser, AST, scopes, diagnostics) | keep: the parser; too big to replace here |
| icu_locale_core | 2.4.0 | 6 | script source text (parser, AST, scopes, diagnostics) | keep: the parser; too big to replace here |
| oxc_ast | 0.124.0 | 6 | script source text (parser, AST, scopes, diagnostics) | keep: the parser; too big to replace here |
| oxc_str | 0.124.0 | 5 | script source text (parser, AST, scopes, diagnostics) | keep: the parser; too big to replace here |
| unicode-normalization | 0.1.26 | 5 | String.prototype.normalize | keep for now: 5 unsafe, char::from_u32_unchecked on Hangul arithmetic; small enough to fork later |
| percent-encoding | 2.3.2 | 4 | script source text (parser, AST, scopes, diagnostics) | keep: the parser; too big to replace here |
| writeable | 0.6.4 | 3 | script source text (parser, AST, scopes, diagnostics) | keep: the parser; too big to replace here |
| bitflags | 2.13.2 | 2 | script source text (parser, AST, scopes, diagnostics) | keep: the parser; too big to replace here |
| either | 1.19.0 | 2 | script source text (parser, AST, scopes, diagnostics) | keep: the parser; too big to replace here |
| icu_segmenter | 2.4.0 | 2 | script source text (parser, AST, scopes, diagnostics) | keep: the parser; too big to replace here |
| serde | 1.0.229 | 2 | script source text (parser, AST, scopes, diagnostics) | keep: the parser; too big to replace here |
| serde_core | 1.0.229 | 2 | script source text (parser, AST, scopes, diagnostics) | keep: the parser; too big to replace here |
| unicode-id-start | 1.5.0 | 2 | script source text (parser, AST, scopes, diagnostics) | keep: the parser; too big to replace here |
| icu_locale_fallback | 2.4.0 | 1 | script source text (parser, AST, scopes, diagnostics) | keep: the parser; too big to replace here |
| num-traits | 0.2.19 | 1 | BigInt, numbers | keep |
| oxc_regular_expression | 0.124.0 | 1 | script source text (parser, AST, scopes, diagnostics) | keep: the parser; too big to replace here |
| regex | 1.13.1 | 1 | RegExp | regexp track (d-strings) replaces it |
| static_assertions | 1.1.0 | 1 | script source text (parser, AST, scopes, diagnostics) | keep: the parser; too big to replace here |
| cfg-if | 1.0.5 | 0 | script source text (parser, AST, scopes, diagnostics) | keep: the parser; too big to replace here |
| equivalent | 1.0.3 | 0 | hashbrown | keep |
| icu_locale_fallback_data | 2.4.0 | 0 | script source text (parser, AST, scopes, diagnostics) | keep: the parser; too big to replace here |
| icu_segmenter_data | 2.4.0 | 0 | script source text (parser, AST, scopes, diagnostics) | keep: the parser; too big to replace here |
| litemap | 0.8.3 | 0 | script source text (parser, AST, scopes, diagnostics) | keep: the parser; too big to replace here |
| num-integer | 0.1.47 | 0 | BigInt | keep |
| owo-colors | 4.4.0 | 0 | script source text (parser, AST, scopes, diagnostics) | keep: the parser; too big to replace here |
| oxc_diagnostics | 0.124.0 | 0 | script source text (parser, AST, scopes, diagnostics) | keep: the parser; too big to replace here |
| oxc_span | 0.124.0 | 0 | script source text (parser, AST, scopes, diagnostics) | keep: the parser; too big to replace here |
| phf | 0.13.1 | 0 | script source text (parser, AST, scopes, diagnostics) | keep: the parser; too big to replace here |
| phf_shared | 0.13.1 | 0 | script source text (parser, AST, scopes, diagnostics) | keep: the parser; too big to replace here |
| regex-syntax | 0.8.11 | 0 | RegExp | regexp track |
| rustc-hash | 2.1.3 | 0 | script source text (parser, AST, scopes, diagnostics) | keep: the parser; too big to replace here |
| small_string | 1.0.0 | 0 | strings | ours, `#![forbid(unsafe_code)]` |
| smawk | 0.3.3 | 0 | script source text (parser, AST, scopes, diagnostics) | keep: the parser; too big to replace here |
| textwrap | 0.16.4 | 0 | script source text (parser, AST, scopes, diagnostics) | keep: the parser; too big to replace here |
| thiserror | 2.0.21 | 0 | script source text (parser, AST, scopes, diagnostics) | keep: the parser; too big to replace here |
| tinyvec | 1.13.3 | 0 | unicode-normalization | keep (0 unsafe) |
| unicode-segmentation | 1.13.3 | 0 | script source text (parser, AST, scopes, diagnostics) | keep: the parser; too big to replace here |
| unicode-width | 0.2.3 | 0 | script source text (parser, AST, scopes, diagnostics) | keep: the parser; too big to replace here |
| wtf8 | 1.0.0 | 0 | strings | ours, `#![forbid(unsafe_code)]` |
| xsum | 0.1.6 | 0 | Math.sumPrecise | keep (0 unsafe) |
| zerofrom | 0.1.8 | 0 | script source text (parser, AST, scopes, diagnostics) | keep: the parser; too big to replace here |

81 crates, 58 with unsafe, 3518 unsafe keywords in all.

With default features the tree also has temporal_rs and its dependencies: bytes, calendrical_calculations,
combine, core_maths, core-foundation-sys, iana-time-zone, icu_calendar, icu_calendar_data, ixdtf, jiff-tzdb,
libm, timezone_provider, tzif, web-time.
