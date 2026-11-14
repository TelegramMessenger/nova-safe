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
