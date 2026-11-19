#![expect(clippy::missing_errors_doc)]
#![forbid(unsafe_code)]

mod ast_impl;
mod diagnostics;
mod options;
mod parser;
mod surrogate_pair;

mod generated {
    // 64-bit only: the recorded 32-bit layouts are upstream's, for its own arena `Vec` (four words). Ours wraps
    // std's (three words), so they do not hold on a 32-bit target, and nothing here depends on a layout.
    #[cfg(all(debug_assertions, target_pointer_width = "64"))]
    mod assert_layouts;
    mod derive_clone_in;
    mod derive_content_eq;
}

pub mod ast;
pub use crate::{
    ast_impl::support::{RegexUnsupportedPatterns, has_unsupported_regular_expression_pattern},
    ast_impl::visit,
    options::Options,
    parser::{ConstructorParser, LiteralParser},
};
