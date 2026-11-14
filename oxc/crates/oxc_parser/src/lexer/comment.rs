use oxc_ast::CommentKind;
use oxc_syntax::line_terminator::is_line_terminator;

use crate::{config::LexerConfig as Config, diagnostics};

use super::{
    Kind, Lexer, cold_branch,
    search::{SafeByteMatchTable, byte_search, safe_byte_match_table},
    source::SourcePosition,
};

// Irregular line breaks - '\u{2028}' (LS) and '\u{2029}' (PS)
const LS_OR_PS_FIRST: u8 = 0xE2;
const LS_BYTES_2_AND_3: [u8; 2] = [0x80, 0xA8];
const PS_BYTES_2_AND_3: [u8; 2] = [0x80, 0xA9];

static LINE_BREAK_TABLE: SafeByteMatchTable =
    safe_byte_match_table!(|b| matches!(b, b'\r' | b'\n' | LS_OR_PS_FIRST));

static MULTILINE_COMMENT_START_TABLE: SafeByteMatchTable =
    safe_byte_match_table!(|b| matches!(b, b'*' | b'\r' | b'\n' | LS_OR_PS_FIRST));

impl<'a, C: Config> Lexer<'a, C> {
    /// Section 12.4 Single Line Comment
    pub(super) fn skip_single_line_comment(&mut self) -> Kind {
        byte_search! {
            lexer: self,
            table: LINE_BREAK_TABLE,
            continue_if: (next_byte, pos) {
                // Match found. Decide whether to continue searching.
                // If this is end of comment, create trivia, and advance `pos` to after line break.
                // Do that here rather than in `handle_match`, to avoid branching twice on value of
                // the matched byte.
                #[expect(clippy::if_not_else)]
                if next_byte != LS_OR_PS_FIRST {
                    // `\r` or `\n`
                    self.trivia_builder
                        .add_line_comment(self.token.start(), self.source.offset_of(pos), self.source.whole());
                    pos = pos.add(1);
                    // We've found the end. Do not continue searching.
                    false
                } else {
                    // `0xE2`. Could be first byte of LS/PS, or could be some other Unicode char.
                    // Either way, Unicode is uncommon, so make this a cold branch.
                    cold_branch(|| {
                        let next2 = pos.add(1).read2();
                        if matches!(next2, LS_BYTES_2_AND_3 | PS_BYTES_2_AND_3) {
                            // Irregular line break
                            self.trivia_builder
                                .add_line_comment(self.token.start(), self.source.offset_of(pos), self.source.whole());
                            // Advance `pos` to after this char.
                            pos = pos.add(3);
                            // We've found the end. Do not continue searching.
                            false
                        } else {
                            // Some other Unicode char beginning with `0xE2`.
                            // Skip 3 bytes (macro skips 1 already, so skip 2 here), and continue searching.
                            pos = pos.add(2);
                            true
                        }
                    })
                }
            },
            handle_eof: {
                self.trivia_builder.add_line_comment(self.token.start(), self.offset(), self.source.whole());
                return Kind::Skip;
            },
        };

        self.token.set_is_on_new_line(true);
        Kind::Skip
    }

    /// Section 12.4 Multi Line Comment
    pub(super) fn skip_multi_line_comment(&mut self) -> Kind {
        // If `is_on_new_line` is already set, go directly to faster search which only looks for `*/`
        // We need to identify if comment contains line breaks or not
        // (`CommentKind::Block` or `CommentKind::MultilineBlock`).
        // So we have to use the loop below for the first line of the comment even if
        // `Token`'s `is_on_new_line` flag is already set.
        // If the loop finds a line break before end of the comment, we then switch to
        // the faster `skip_multi_line_comment_after_line_break` which searches
        // for the end of the comment.

        byte_search! {
            lexer: self,
            table: MULTILINE_COMMENT_START_TABLE,
            continue_if: (next_byte, pos) {
                // Match found. Decide whether to continue searching.
                if next_byte == b'*' {
                    let after_star = pos.add(1);
                    if after_star.is_not_end_of(&self.source) {
                        // If next byte isn't `/`, continue
                        if after_star.read() == b'/' {
                            // Consume `*/`
                            pos = pos.add(2);
                            false
                        } else {
                            true
                        }
                    } else {
                        // This is last byte in file. Continue to `handle_eof`.
                        // This is illegal in valid JS, so mark this branch cold.
                        cold_branch(|| true)
                    }
                } else if next_byte == LS_OR_PS_FIRST {
                    // `0xE2`. Could be first byte of LS/PS, or could be some other Unicode char.
                    // Either way, Unicode is uncommon, so make this a cold branch.
                    cold_branch(|| {
                        let next2 = pos.add(1).read2();
                        if matches!(next2, LS_BYTES_2_AND_3 | PS_BYTES_2_AND_3) {
                            // Irregular line break
                            self.token.set_is_on_new_line(true);
                            // Ideally we'd go on to `skip_multi_line_comment_after_line_break` here
                            // but can't do that easily because can't use `return` in a closure.
                            // But irregular line breaks are rare anyway.
                        }
                        // Either way, continue searching.
                        // Skip 3 bytes (macro skips 1 already, so skip 2 here), and continue searching.
                        pos = pos.add(2);
                        true
                    })
                } else {
                    // Regular line break.
                    // No need to look for more line breaks, so switch to faster search just for `*/`.
                    self.token.set_is_on_new_line(true);
                    let after_line_break = pos.add(1);
                    return self.skip_multi_line_comment_after_line_break(after_line_break);
                }
            },
            handle_eof: {
                self.error(diagnostics::unterminated_multi_line_comment(self.unterminated_range()));
                return Kind::Eof;
            },
        };

        self.trivia_builder.add_block_comment(
            self.token.start(),
            self.offset(),
            CommentKind::SingleLineBlock,
            self.source.whole(),
        );
        Kind::Skip
    }

    fn skip_multi_line_comment_after_line_break(&mut self, pos: SourcePosition<'a>) -> Kind {
        if let Some(index) = find_star_slash(pos.rest()) {
            self.source.set_position(pos.add(index + 2));
            self.trivia_builder.add_block_comment(
                self.token.start(),
                self.offset(),
                CommentKind::MultiLineBlock,
                self.source.whole(),
            );
            Kind::Skip
        } else {
            self.source.advance_to_end();
            self.error(diagnostics::unterminated_multi_line_comment(self.unterminated_range()));
            Kind::Eof
        }
    }

    /// Section 12.5 Hashbang Comments.
    ///
    /// Next 2 bytes must be `#!`.
    pub(super) fn read_hashbang_comment(&mut self) -> Kind {
        debug_assert!(self.peek_2_bytes() == Some([b'#', b'!']));

        self.source.next_byte();
        self.source.next_byte();

        while let Some(c) = self.peek_char() {
            if is_line_terminator(c) {
                break;
            }
            self.consume_char();
        }
        Kind::HashbangComment
    }
}

/// Find the first `*/` in `bytes`, returning the offset of the `*`.
fn find_star_slash(bytes: &[u8]) -> Option<usize> {
    let mut from = 0;
    while let Some(index) = bytes.get(from..)?.iter().position(|&b| b == b'*') {
        let star = from + index;
        if bytes.get(star + 1) == Some(&b'/') {
            return Some(star);
        }
        from = star + 1;
    }
    None
}
