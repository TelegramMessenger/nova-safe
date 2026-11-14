// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! A scan of source text that bounds how deeply its syntax tree can nest,
//! before the parser (which recurses once per level and cannot be stopped)
//! sees it.
//!
//! The estimate counts bracket levels plus, inside each level, the operators
//! and nesting keywords since the last `,` or `;` (or the end of a block
//! followed by a new statement): an expression or statement chain cannot
//! nest more deeply than it has operators. Strings, comments, template text
//! and regular expression literals are skipped.

/// Deepest nesting estimate a source text may have.
pub(crate) const MAX_SOURCE_NESTING: usize = 3000;

struct Level {
    kind: u8,
    ops: usize,
    elses: usize,
}

fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'$' || b >= 0x80
}

const NESTING_KEYWORDS: &[&[u8]] = &[
    b"typeof",
    b"void",
    b"delete",
    b"await",
    b"new",
    b"yield",
    b"if",
    b"for",
    b"while",
    b"with",
    b"do",
    b"in",
    b"instanceof",
    b"of",
    b"return",
    b"throw",
    b"case",
];

const REGEX_AFTER_KEYWORDS: &[&[u8]] = &[
    b"return",
    b"typeof",
    b"instanceof",
    b"in",
    b"of",
    b"new",
    b"delete",
    b"void",
    b"throw",
    b"case",
    b"do",
    b"else",
    b"yield",
    b"await",
];

/// True if the nesting estimate of `source` is over [`MAX_SOURCE_NESTING`].
pub(crate) fn source_nesting_too_deep(source: &str) -> bool {
    source_nesting_depth(source.as_bytes(), MAX_SOURCE_NESTING) > MAX_SOURCE_NESTING
}

/// The nesting estimate of `src`, stopping early once over `limit`.
pub(crate) fn source_nesting_depth(src: &[u8], limit: usize) -> usize {
    let mut levels = vec![Level {
        kind: b'S',
        ops: 0,
        elses: 0,
    }];
    let mut current = 1usize;
    let mut max = 1usize;
    // True when a `/` here starts a regular expression literal.
    let mut regex_allowed = true;
    // A `}` closed a level: the next token says if a new statement starts.
    let mut after_close_brace = false;
    let mut in_template = false;
    let mut i = 0usize;
    let n = src.len();
    macro_rules! level {
        () => {
            levels
                .last_mut()
                .expect("the outermost level is never popped")
        };
    }
    while i < n {
        if in_template {
            match src[i] {
                b'\\' => i += 2,
                b'`' => {
                    in_template = false;
                    regex_allowed = false;
                    i += 1;
                }
                b'$' if src.get(i + 1) == Some(&b'{') => {
                    in_template = false;
                    levels.push(Level {
                        kind: b'T',
                        ops: 0,
                        elses: 0,
                    });
                    current += 1;
                    max = max.max(current);
                    regex_allowed = true;
                    i += 2;
                }
                _ => i += 1,
            }
            continue;
        }
        let b = src[i];
        if b.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if b == b'/' && src.get(i + 1) == Some(&b'/') {
            while i < n && src[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if b == b'/' && src.get(i + 1) == Some(&b'*') {
            i += 2;
            while i + 1 < n && !(src[i] == b'*' && src[i + 1] == b'/') {
                i += 1;
            }
            i += 2;
            continue;
        }
        if after_close_brace {
            after_close_brace = false;
            let starts_word = is_ident_byte(b) && !b.is_ascii_digit();
            let is_else =
                src[i..].starts_with(b"else") && !src.get(i + 4).is_some_and(|&c| is_ident_byte(c));
            if starts_word && !is_else {
                let level = level!();
                current -= level.ops;
                level.ops = 0;
            }
        }
        match b {
            b'"' | b'\'' => {
                i += 1;
                while i < n && src[i] != b {
                    if src[i] == b'\\' {
                        i += 1;
                    } else if src[i] == b'\n' {
                        break;
                    }
                    i += 1;
                }
                i += 1;
                regex_allowed = false;
            }
            b'`' => {
                in_template = true;
                i += 1;
            }
            b'(' | b'[' | b'{' => {
                levels.push(Level {
                    kind: b,
                    ops: 0,
                    elses: 0,
                });
                current += 1;
                max = max.max(current);
                regex_allowed = true;
                i += 1;
            }
            b')' | b']' | b'}' => {
                if levels.len() > 1 {
                    let level = levels.pop().expect("checked above");
                    current -= 1 + level.ops + level.elses;
                    if level.kind == b'T' && b == b'}' {
                        in_template = true;
                    }
                }
                regex_allowed = b == b'}' && !in_template;
                after_close_brace = b == b'}';
                i += 1;
            }
            b',' | b';' => {
                let level = level!();
                current -= level.ops;
                level.ops = 0;
                regex_allowed = true;
                i += 1;
            }
            b'/' if regex_allowed => {
                i += 1;
                let mut in_class = false;
                while i < n && src[i] != b'\n' {
                    match src[i] {
                        b'\\' => i += 1,
                        b'[' => in_class = true,
                        b']' => in_class = false,
                        b'/' if !in_class => break,
                        _ => {}
                    }
                    i += 1;
                }
                i += 1;
                while i < n && is_ident_byte(src[i]) {
                    i += 1;
                }
                regex_allowed = false;
            }
            _ if is_ident_byte(b) => {
                let start = i;
                while i < n && is_ident_byte(src[i]) {
                    i += 1;
                }
                let word = &src[start..i];
                if word == b"else" {
                    level!().elses += 1;
                    current += 1;
                } else if NESTING_KEYWORDS.contains(&word) {
                    level!().ops += 1;
                    current += 1;
                }
                regex_allowed = REGEX_AFTER_KEYWORDS.contains(&word);
            }
            _ => {
                // Every operator character may add a level: `!!!!x`,
                // `a=b=c`, `a?b:c?d:e`, `a+b+c` and `a.b.c` all nest.
                level!().ops += 1;
                current += 1;
                regex_allowed = !(b == b'.' && src.get(i + 1).is_some_and(|c| !c.is_ascii_digit()));
                i += 1;
            }
        }
        max = max.max(current);
        if max > limit {
            return max;
        }
    }
    max
}
