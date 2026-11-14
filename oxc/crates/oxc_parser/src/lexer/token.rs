//! Token

use std::fmt;

use oxc_span::Span;

use super::kind::Kind;

/// Lexed token: span, kind and flags. 16 bytes.
#[derive(Clone, Copy, Default)]
pub struct Token {
    start: u32,
    end: u32,
    kind: Kind,
    is_on_new_line: bool,
    escaped: bool,
    lone_surrogates: bool,
    has_separator: bool,
}

impl fmt::Debug for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Token")
            .field("kind", &self.kind())
            .field("start", &self.start())
            .field("end", &self.end())
            .field("is_on_new_line", &self.is_on_new_line())
            .field("escaped", &self.escaped())
            .field("lone_surrogates", &self.lone_surrogates())
            .field("has_separator", &self.has_separator())
            .finish()
    }
}

impl Token {
    #[inline]
    pub(super) fn new_on_new_line() -> Self {
        Self { is_on_new_line: true, ..Self::default() }
    }
}

// Getters and setters
impl Token {
    #[inline]
    pub fn span(&self) -> Span {
        Span::new(self.start(), self.end())
    }

    // `set_span` is only exposed as public API when `mutate_tokens` feature is enabled.
    // Otherwise, it is only accessible within `lexer` module.
    #[cfg(feature = "mutate_tokens")]
    #[inline]
    pub fn set_span(&mut self, span: Span) {
        self.set_span_impl(span);
    }

    #[cfg(not(feature = "mutate_tokens"))]
    #[inline]
    #[allow(dead_code, clippy::allow_attributes)]
    pub(super) fn set_span(&mut self, span: Span) {
        self.set_span_impl(span);
    }

    #[inline]
    fn set_span_impl(&mut self, span: Span) {
        self.set_start(span.start);
        self.set_end(span.end);
    }

    #[inline]
    pub fn start(&self) -> u32 {
        self.start
    }

    #[inline]
    pub(super) fn set_start(&mut self, start: u32) {
        self.start = start;
    }

    #[inline]
    pub fn end(&self) -> u32 {
        self.end
    }

    #[inline]
    pub(super) fn set_end(&mut self, end: u32) {
        let start = self.start();
        debug_assert!(end >= start, "Token end ({end}) cannot be less than start ({start})");
        self.end = end;
    }

    #[inline]
    pub fn kind(&self) -> Kind {
        self.kind
    }

    // `set_kind` is only exposed as public API when `mutate_tokens` feature is enabled.
    // Otherwise, it is only accessible within `lexer` module.
    #[cfg(feature = "mutate_tokens")]
    #[inline]
    pub fn set_kind(&mut self, kind: Kind) {
        self.set_kind_impl(kind);
    }

    #[cfg(not(feature = "mutate_tokens"))]
    #[inline]
    pub(super) fn set_kind(&mut self, kind: Kind) {
        self.set_kind_impl(kind);
    }

    #[inline]
    fn set_kind_impl(&mut self, kind: Kind) {
        self.kind = kind;
    }

    #[inline]
    pub fn is_on_new_line(&self) -> bool {
        self.is_on_new_line
    }

    #[inline]
    pub(super) fn set_is_on_new_line(&mut self, value: bool) {
        self.is_on_new_line = value;
    }

    #[inline]
    pub fn escaped(&self) -> bool {
        self.escaped
    }

    #[inline]
    pub(super) fn set_escaped(&mut self, escaped: bool) {
        self.escaped = escaped;
    }

    #[inline]
    pub fn lone_surrogates(&self) -> bool {
        self.lone_surrogates
    }

    #[inline]
    pub(super) fn set_lone_surrogates(&mut self, value: bool) {
        self.lone_surrogates = value;
    }

    #[inline]
    pub fn has_separator(&self) -> bool {
        self.has_separator
    }

    #[inline]
    pub(super) fn set_has_separator(&mut self, value: bool) {
        self.has_separator = value;
    }
}

#[cfg(test)]
mod test {
    use super::{Kind, Span, Token};

    // Test size of `Token`
    const _: () = assert!(size_of::<Token>() == 16);

    // Test default token values
    #[test]
    fn default_token_values() {
        let token = Token::default();
        assert_eq!(token.start(), 0);
        assert_eq!(token.end(), 0);
        assert_eq!(token.kind(), Kind::Eof); // Kind::default() is Eof
        assert!(!token.is_on_new_line());
        assert!(!token.escaped());
        assert!(!token.lone_surrogates());
        assert!(!token.has_separator());
    }

    #[test]
    fn new_on_new_line_token_values() {
        let token = Token::new_on_new_line();
        assert_eq!(token.start(), 0);
        assert_eq!(token.end(), 0);
        assert_eq!(token.kind(), Kind::Eof);
        assert!(token.is_on_new_line());
        assert!(!token.escaped());
        assert!(!token.lone_surrogates());
        assert!(!token.has_separator());
    }

    #[test]
    fn token_creation_and_retrieval() {
        let kind = Kind::Ident;
        let start = 100u32;
        let end = start + 5u32;
        let is_on_new_line = true;
        let escaped = false;
        let lone_surrogates = true;
        let has_separator = false;

        let mut token = Token::default();
        token.set_kind(kind);
        token.set_start(start);
        token.set_end(end);
        token.set_is_on_new_line(is_on_new_line);
        token.set_escaped(escaped);
        token.set_lone_surrogates(lone_surrogates);
        if has_separator {
            // Assuming set_has_separator is not always called if false
            token.set_has_separator(true);
        }

        assert_eq!(token.kind(), kind);
        assert_eq!(token.start(), start);
        assert_eq!(token.end(), end);
        assert_eq!(token.is_on_new_line(), is_on_new_line);
        assert_eq!(token.escaped(), escaped);
        assert_eq!(token.lone_surrogates(), lone_surrogates);
        assert_eq!(token.has_separator(), has_separator);
    }

    #[test]
    fn token_setters() {
        let mut token = Token::default();
        token.set_kind(Kind::Ident);
        token.set_span(Span::new(10, 15));
        // is_on_new_line, escaped, lone_surrogates, has_separator are false by default from Token::default()

        assert_eq!(token.start(), 10);
        assert_eq!(token.end(), 15);
        assert!(!token.escaped());
        assert!(!token.is_on_new_line());
        assert!(!token.lone_surrogates());

        // Test set_end
        let mut token_for_set_end = Token::default();
        token_for_set_end.set_kind(Kind::Ident);
        token_for_set_end.set_start(10);
        token_for_set_end.set_end(15);

        assert_eq!(token_for_set_end.end(), 15);
        token_for_set_end.set_end(30);
        assert_eq!(token_for_set_end.start(), 10);
        assert_eq!(token_for_set_end.end(), 30);

        // Test that other flags are not affected by set_start
        let mut token_with_flags = Token::default();
        token_with_flags.set_kind(Kind::Str);
        token_with_flags.set_start(30);
        token_with_flags.set_end(33);
        token_with_flags.set_is_on_new_line(true);
        token_with_flags.set_escaped(true);
        token_with_flags.set_lone_surrogates(true);
        token_with_flags.set_has_separator(true);

        token_with_flags.set_start(40);
        assert_eq!(token_with_flags.start(), 40);
        assert!(token_with_flags.is_on_new_line());
        assert!(token_with_flags.escaped());
        assert!(token_with_flags.lone_surrogates());
        assert!(token_with_flags.has_separator());

        // Test that other flags are not affected by set_escaped
        let mut token_with_flags2 = Token::default();
        token_with_flags2.set_kind(Kind::Str);
        token_with_flags2.set_start(50);
        token_with_flags2.set_end(52);
        token_with_flags2.set_is_on_new_line(true);
        // escaped is false by default
        token_with_flags2.set_lone_surrogates(true);
        token_with_flags2.set_has_separator(true);

        token_with_flags2.set_escaped(true);
        assert_eq!(token_with_flags2.start(), 50);
        assert!(token_with_flags2.is_on_new_line());
        assert!(token_with_flags2.escaped());
        assert!(token_with_flags2.lone_surrogates());
        assert!(token_with_flags2.has_separator());
        token_with_flags2.set_escaped(false);
        assert!(!token_with_flags2.escaped());
        assert!(token_with_flags2.is_on_new_line()); // Check again
        assert!(token_with_flags2.lone_surrogates()); // Check again
        assert!(token_with_flags2.has_separator()); // Check again

        // Test set_is_on_new_line does not affect other flags
        let mut token_flags_test_newline = Token::default();
        token_flags_test_newline.set_kind(Kind::Str);
        token_flags_test_newline.set_start(60);
        token_flags_test_newline.set_end(62);
        // is_on_new_line is false by default
        token_flags_test_newline.set_escaped(true);
        token_flags_test_newline.set_lone_surrogates(true);
        token_flags_test_newline.set_has_separator(true);

        token_flags_test_newline.set_is_on_new_line(true);
        assert!(token_flags_test_newline.is_on_new_line());
        assert_eq!(token_flags_test_newline.start(), 60);
        assert!(token_flags_test_newline.escaped());
        assert!(token_flags_test_newline.lone_surrogates());
        assert!(token_flags_test_newline.has_separator());
        token_flags_test_newline.set_is_on_new_line(false);
        assert!(!token_flags_test_newline.is_on_new_line());
        assert!(token_flags_test_newline.escaped());
        assert!(token_flags_test_newline.lone_surrogates());
        assert!(token_flags_test_newline.has_separator());

        // Test set_lone_surrogates does not affect other flags
        let mut token_flags_test_lone_surrogates = Token::default();
        token_flags_test_lone_surrogates.set_kind(Kind::Str);
        token_flags_test_lone_surrogates.set_start(70);
        token_flags_test_lone_surrogates.set_end(72);
        token_flags_test_lone_surrogates.set_is_on_new_line(true);
        token_flags_test_lone_surrogates.set_escaped(true);
        // lone_surrogates is false by default
        token_flags_test_lone_surrogates.set_has_separator(true);

        token_flags_test_lone_surrogates.set_lone_surrogates(true);
        assert!(token_flags_test_lone_surrogates.lone_surrogates());
        assert_eq!(token_flags_test_lone_surrogates.start(), 70);
        assert!(token_flags_test_lone_surrogates.is_on_new_line());
        assert!(token_flags_test_lone_surrogates.escaped());
        assert!(token_flags_test_lone_surrogates.has_separator());
        token_flags_test_lone_surrogates.set_lone_surrogates(false);
        assert!(!token_flags_test_lone_surrogates.lone_surrogates());
        assert!(token_flags_test_lone_surrogates.is_on_new_line());
        assert!(token_flags_test_lone_surrogates.escaped());
        assert!(token_flags_test_lone_surrogates.has_separator());
    }

    #[test]
    fn is_on_new_line() {
        let mut token = Token::default();
        assert!(!token.is_on_new_line());
        token.set_is_on_new_line(true);
        assert!(token.is_on_new_line());
        token.set_is_on_new_line(false);
        assert!(!token.is_on_new_line());
    }

    #[test]
    fn escaped() {
        let mut token = Token::default();
        assert!(!token.escaped());
        token.set_escaped(true);
        assert!(token.escaped());
        token.set_escaped(false);
        assert!(!token.escaped());
    }

    #[test]
    fn lone_surrogates() {
        let mut token = Token::default();
        assert!(!token.lone_surrogates());
        token.set_lone_surrogates(true);
        assert!(token.lone_surrogates());
        token.set_lone_surrogates(false);
        assert!(!token.lone_surrogates());
    }

    #[test]
    fn has_separator() {
        let mut token = Token::default();
        assert!(!token.has_separator());
        token.set_has_separator(true);
        assert!(token.has_separator());
        token.set_has_separator(false);
        assert!(!token.has_separator());
    }
}
