//! A script chooses every name the parser and the semantic checker put in a
//! hash map. These tests check that names built to collide under upstream
//! oxc's unkeyed identifier hash (same length, same sampled bytes) parse in
//! about the time of as many ordinary names, and that diagnostics that walk
//! such maps come out in source order whatever the random keys are.

use std::time::{Duration, Instant};

use oxc_allocator::Allocator;
use oxc_parser::Parser;
use oxc_regular_expression::{LiteralParser, Options};
use oxc_semantic::SemanticBuilder;
use oxc_span::SourceType;

const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ";

/// The `i`th of a family of 40-byte names. With `colliding`, the names differ
/// only in bytes 4..8, which upstream's hash never read, so they all had one
/// hash; otherwise they differ in bytes 0..4.
fn name(i: usize, colliding: bool, tag: u8) -> String {
    let mut varying = [0u8; 4];
    let mut n = i;
    for byte in varying.iter_mut().rev() {
        *byte = ALPHABET[n % ALPHABET.len()];
        n /= ALPHABET.len();
    }
    let varying = std::str::from_utf8(&varying).unwrap();
    let fixed = format!("ABC{}", tag as char);
    let tail = "EFGHIJJJJJJJJJJJJJJJJJJJJJJJJJJJ";
    if colliding {
        format!("{fixed}{varying}{tail}")
    } else {
        format!("{varying}{fixed}{tail}")
    }
}

/// `count` declared names, as many unresolved references, and a function
/// whose parameters and locals use the same family.
fn script(count: usize, colliding: bool) -> String {
    let mut source = String::new();
    for i in 0..count {
        source.push_str(&format!("var {};\n", name(i, colliding, b'D')));
    }
    for i in 0..count {
        source.push_str(&format!("typeof {};\n", name(i, colliding, b'U')));
    }
    source.push_str("function f() {\n");
    for i in 0..count / 4 {
        source.push_str(&format!(
            "let {} = {};\n",
            name(i, colliding, b'L'),
            name(i, colliding, b'D')
        ));
    }
    source.push_str("}\n");
    source
}

fn parse_and_check(source: &str, source_type: SourceType) -> Vec<String> {
    let allocator = Allocator::new();
    let ret = Parser::new(&allocator, source, source_type).parse();
    let mut errors: Vec<String> = ret.errors.iter().map(|e| e.message.to_string()).collect();
    let semantic = SemanticBuilder::new()
        .with_check_syntax_error(true)
        .build(&ret.program);
    errors.extend(semantic.errors.iter().map(|e| {
        let mut text = e.message.to_string();
        if let Some(help) = &e.help {
            text.push_str(" | ");
            text.push_str(help);
        }
        text
    }));
    errors
}

fn best_time(source: &str) -> Duration {
    (0..3)
        .map(|_| {
            let started = Instant::now();
            let errors = parse_and_check(source, SourceType::script());
            let elapsed = started.elapsed();
            assert!(errors.is_empty(), "{:?}", errors.first());
            elapsed
        })
        .min()
        .unwrap()
}

#[test]
fn names_built_to_collide_parse_in_linear_time() {
    assert_eq!(name(0, true, b'D').len(), name(1, true, b'D').len());
    let count = 20_000;
    let colliding = best_time(&script(count, true));
    let ordinary = best_time(&script(count, false));
    // Under the unkeyed hash this took over 100 times as long as ordinary
    // names (quadratic in the count); now the two are about equal.
    assert!(
        colliding < ordinary * 4 + Duration::from_millis(500),
        "{count} colliding names: {colliding:?}, ordinary names: {ordinary:?}"
    );
}

#[test]
fn import_redeclarations_are_reported_in_source_order() {
    let count = 200;
    let mut source = String::from("import {");
    for i in 0..count {
        source.push_str(&format!("n{i}, "));
    }
    source.push_str("} from 'm';\n");
    for i in 0..count {
        source.push_str(&format!("var n{i}; var n{i};\n"));
    }
    let allocator = Allocator::new();
    let ret = Parser::new(&allocator, &source, SourceType::mjs()).parse();
    let semantic = SemanticBuilder::new()
        .with_check_syntax_error(true)
        .build(&ret.program);
    let errors = ret.errors.iter().chain(semantic.errors.iter());
    let mut starts = Vec::new();
    for error in errors {
        let label = error.labels.as_ref().and_then(|labels| labels.first());
        starts.push(label.map(|label| label.offset()).unwrap_or(0));
    }
    assert!(starts.len() >= count, "{} errors", starts.len());
    let mut sorted = starts.clone();
    sorted.sort_unstable();
    assert_eq!(starts, sorted);
}

#[test]
fn undefined_export_suggests_the_first_close_name() {
    let mut source = String::new();
    let suffixes: Vec<char> = ('a'..='z').chain('A'..='Z').chain('0'..='9').collect();
    for c in suffixes.iter().rev() {
        source.push_str(&format!("let k{c};\n"));
    }
    source.push_str("export { k_ };\n");
    let errors = parse_and_check(&source, SourceType::mjs());
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(errors[0].ends_with("Did you mean 'k9'?"), "{errors:?}");
}

#[test]
fn invalid_named_reference_lists_groups_in_source_order() {
    let names: Vec<String> = (0..60).map(|i| format!("g{}", 59 - i)).collect();
    let mut pattern = String::new();
    for name in &names {
        pattern.push_str(&format!("(?<{name}>.)"));
    }
    pattern.push_str("\\k<nope>");
    let allocator = Allocator::new();
    let error = LiteralParser::new(&allocator, &pattern, Some("u"), Options::default())
        .parse()
        .unwrap_err();
    let help = error.help.as_deref().unwrap_or_default();
    assert_eq!(help, format!("Valid group names are: {}", names.join(", ")));
}

fn best_time_of(source: &str, source_type: SourceType) -> Duration {
    (0..3)
        .map(|_| {
            let started = Instant::now();
            let errors = parse_and_check(source, source_type);
            let elapsed = started.elapsed();
            assert!(errors.is_empty(), "{:?}", errors.first());
            elapsed
        })
        .min()
        .unwrap()
}

fn private_fields_each_read_once(n: usize) -> String {
    let mut source = String::from("class C {");
    for i in 0..n {
        source.push_str(&format!("#p{i} = 0;"));
    }
    source.push_str("m() {");
    for i in 0..n {
        source.push_str(&format!("this.#p{i};"));
    }
    source.push_str("} }");
    source
}

fn imports_exported_again(n: usize) -> String {
    let names: Vec<String> = (0..n).map(|i| format!("a{i}")).collect();
    format!(
        "import {{ {} }} from 'x';\nexport {{ {} }};\n",
        names.join(", "),
        names.join(", ")
    )
}

fn regexp_time(pattern: &str) -> Duration {
    (0..3)
        .map(|_| {
            let allocator = Allocator::new();
            let started = Instant::now();
            let result = LiteralParser::new(&allocator, pattern, None, Options::default()).parse();
            let elapsed = started.elapsed();
            assert!(
                result.is_ok(),
                "{:?}",
                result.err().map(|e| e.message.to_string())
            );
            elapsed
        })
        .min()
        .unwrap()
}

/// Each private name reference, each export of an imported name and each
/// regexp group name used to be compared with every earlier one: four times
/// the names took sixteen times as long.
#[test]
fn checks_over_many_names_scale_linearly() {
    let linear = |small: Duration, big: Duration, what: &str| {
        assert!(
            big < small * 8 + Duration::from_millis(100),
            "{what}: {small:?} for n, {big:?} for 4n"
        );
    };
    linear(
        best_time_of(&private_fields_each_read_once(5_000), SourceType::script()),
        best_time_of(&private_fields_each_read_once(20_000), SourceType::script()),
        "private names",
    );
    linear(
        best_time_of(&imports_exported_again(5_000), SourceType::mjs()),
        best_time_of(&imports_exported_again(20_000), SourceType::mjs()),
        "exports of imports",
    );
    let distinct = |n: usize| (0..n).map(|i| format!("(?<g{i}>.)")).collect::<String>();
    linear(
        regexp_time(&distinct(5_000)),
        regexp_time(&distinct(20_000)),
        "distinct group names",
    );
    let alternatives = |n: usize| vec!["(?<a>.)"; n].join("|");
    linear(
        regexp_time(&alternatives(5_000)),
        regexp_time(&alternatives(20_000)),
        "one group name in many alternatives",
    );
}

/// `class extends ,` used to label its error with a span that ends before it
/// starts, which overflowed in builds with overflow checks.
#[test]
fn a_missing_heritage_expression_does_not_overflow_a_span() {
    for source_type in [SourceType::script(), SourceType::mjs()] {
        for source in [
            "//\nx=class extends, ",
            "x=class extends, ",
            "class A extends , B {}",
        ] {
            let allocator = Allocator::new();
            let ret = Parser::new(&allocator, source, source_type).parse();
            assert!(!ret.errors.is_empty(), "{source:?}");
            for error in &ret.errors {
                for label in error.labels.iter().flatten() {
                    assert!(label.offset() + label.len() <= source.len(), "{source:?}");
                }
            }
        }
    }
}

/// Binary, octal and hex BigInt literals carry their decimal value.
#[test]
fn bigint_literals_carry_their_decimal_value() {
    use num_bigint::BigUint;
    use num_traits::Num;
    let digits = "f0e1d2c3b4a5968778695a4b3c2d1e0f".repeat(40);
    let cases = [
        ("0x0n".to_owned(), "0".to_owned()),
        ("0b1_0000_0000n".to_owned(), "256".to_owned()),
        ("0o777n".to_owned(), "511".to_owned()),
        (
            "0xffff_ffff_ffff_ffffn".to_owned(),
            "18446744073709551615".to_owned(),
        ),
        (
            format!("0x{digits}n"),
            BigUint::from_str_radix(&digits, 16).unwrap().to_string(),
        ),
    ];
    for (literal, expected) in cases {
        let source = format!("x = {literal};");
        let allocator = Allocator::new();
        let ret = Parser::new(&allocator, &source, SourceType::script()).parse();
        assert!(ret.errors.is_empty());
        let oxc_ast::ast::Statement::ExpressionStatement(statement) = &ret.program.body[0] else {
            panic!("{source}");
        };
        let oxc_ast::ast::Expression::AssignmentExpression(assignment) = &statement.expression
        else {
            panic!("{source}");
        };
        let oxc_ast::ast::Expression::BigIntLiteral(value) = &assignment.right else {
            panic!("{source}");
        };
        assert_eq!(value.value.as_str(), expected, "{literal}");
    }
}
