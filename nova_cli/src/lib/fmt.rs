// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Formatting values and errors.

use nova_vm::{
    ecmascript::{Agent, JsResult, Value},
    engine::{Bindable, GcScope},
};
use oxc_diagnostics::OxcDiagnostic;

pub fn print_result(agent: &mut Agent, result: JsResult<Value>, verbose: bool, gc: GcScope) {
    match result {
        Ok(result) => {
            if verbose {
                println!("{result:?}");
            }
        }
        Err(_) if agent.termination().is_some() => {
            let reason = agent.take_termination().unwrap();
            eprintln!("Terminated: {}", reason.message());
            std::process::exit(1);
        }
        Err(error) => {
            eprintln!(
                "Uncaught exception: {}",
                error
                    .value()
                    .unbind()
                    .string_repr(agent, gc)
                    .as_wtf8(agent)
                    .to_string_lossy()
            );
            std::process::exit(1);
        }
    }
}

/// Exit the program with parse errors: each error's message, then the
/// position, source line and underline of each of its labels.
pub fn exit_with_parse_errors(errors: Vec<OxcDiagnostic>, source_path: &str, source: &str) -> ! {
    assert!(!errors.is_empty());

    eprintln!("SyntaxError:");

    for error in errors {
        eprintln!("  x {}", error.message);
        for label in error.labels.iter().flatten() {
            print_label(source_path, source, label.offset(), label.len(), label.label());
        }
        if let Some(help) = &error.help {
            eprintln!("  help: {help}");
        }
        if let Some(note) = &error.note {
            eprintln!("  note: {note}");
        }
    }

    std::process::exit(1);
}

fn print_label(source_path: &str, source: &str, offset: usize, len: usize, label: Option<&str>) {
    let offset = floor_char_boundary(source, offset);
    let line_start = source[..offset].rfind('\n').map_or(0, |i| i + 1);
    let line_end = source[offset..].find('\n').map_or(source.len(), |i| offset + i);
    let line = source[line_start..line_end].trim_end_matches('\r');
    let line_number = source[..line_start].matches('\n').count() + 1;
    let column = source[line_start..offset].chars().count() + 1;
    eprintln!("   ,-[{source_path}:{line_number}:{column}]");
    eprintln!("   | {line}");
    let indent = source[line_start..offset].chars().count();
    let end = floor_char_boundary(source, offset.saturating_add(len)).min(line_start + line.len());
    let width = source[offset..end.max(offset)].chars().count().max(1);
    match label {
        Some(label) => eprintln!("   | {}{} {label}", " ".repeat(indent), "^".repeat(width)),
        None => eprintln!("   | {}{}", " ".repeat(indent), "^".repeat(width)),
    }
}

fn floor_char_boundary(source: &str, index: usize) -> usize {
    let mut index = index.min(source.len());
    while !source.is_char_boundary(index) {
        index -= 1;
    }
    index
}
