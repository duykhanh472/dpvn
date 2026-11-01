//! Lexer for the format-preview markup subset (Phase 9, Appendix B).
//!
//! The lexer is deliberately line-aware for block markup and byte-oriented for
//! inline markup. It never fails: ambiguous or malformed input is emitted as
//! plain text so the parser can render it literally.

/// Maximum inline nesting depth. Markup opened beyond this level is literal.
pub const MAX_NESTING_DEPTH: usize = 8;

/// The kind of emphasis opened by a delimiter pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmphasisKind {
    /// `_text_`
    Italic,
    /// `*text*`
    Bold,
    /// `=_text_=`
    BoldItalic,
}

/// Block classification for one physical line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineKind<'a> {
    /// An empty or whitespace-only line: a paragraph break.
    Blank,
    /// `----` (four or more hyphens, nothing else).
    HorizontalRule,
    /// `# text` at line start; holds the content after the marker.
    Blockquote(&'a str),
    /// `/>` at line start; holds the remainder including leading spaces.
    Poetry(&'a str),
    /// Ordinary paragraph text.
    Paragraph(&'a str),
}

/// One inline token.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InlineToken<'a> {
    /// Literal text, already unescaped and belonging to no markup.
    Text(&'a str),
    /// An emphasis delimiter. `can_open`/`can_close` encode flanking rules.
    EmphasisMarker {
        /// Which emphasis variant this delimiter belongs to.
        kind: EmphasisKind,
        /// The exact source bytes of the delimiter (`_`, `*`, `=_`, or `_=`).
        marker: &'a str,
        /// Whether the delimiter may open emphasis here.
        can_open: bool,
        /// Whether the delimiter may close emphasis here.
        can_close: bool,
    },
    /// `<sc>` (case-insensitive).
    SmallCapsOpen,
    /// `</sc>` (case-insensitive).
    SmallCapsClose,
    /// A complete `[Footnote N: text]` span.
    Footnote {
        /// Footnote number as written in the source.
        number: u32,
        /// Raw inline content of the footnote.
        content: &'a str,
    },
}

/// Classify a physical line for block parsing.
#[must_use]
pub fn classify_line(line: &str) -> LineKind<'_> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return LineKind::Blank;
    }

    if trimmed.len() >= 4 && trimmed.bytes().all(|byte| byte == b'-') {
        return LineKind::HorizontalRule;
    }

    if let Some(rest) = line.strip_prefix("# ") {
        return LineKind::Blockquote(rest);
    }
    if line == "#" {
        return LineKind::Blockquote("");
    }

    if let Some(rest) = line.strip_prefix("/>") {
        return LineKind::Poetry(rest);
    }

    LineKind::Paragraph(line)
}

/// Tokenize one paragraph (or poetry line) into inline tokens.
#[must_use]
pub fn tokenize_inline(input: &str) -> Vec<InlineToken<'_>> {
    let mut tokens = Vec::new();
    let mut text_start = 0;
    let mut index = 0;

    while index < input.len() {
        let Some(ch) = input[index..].chars().next() else {
            break;
        };

        let candidate = match ch {
            '_' => emphasis_marker(input, index),
            '*' => bold_marker(input, index),
            '=' => bold_italic_open(input, index),
            '<' => small_caps(input, index),
            '[' => footnote(input, index),
            _ => None,
        };

        if let Some((token, length)) = candidate {
            if text_start < index {
                tokens.push(InlineToken::Text(&input[text_start..index]));
            }
            tokens.push(token);
            index += length;
            text_start = index;
        } else {
            index += ch.len_utf8();
        }
    }

    if text_start < input.len() {
        tokens.push(InlineToken::Text(&input[text_start..]));
    }

    tokens
}

/// Prev/next scalar helpers used for emphasis flanking rules.
fn prev_char(input: &str, index: usize) -> Option<char> {
    input[..index].chars().next_back()
}

fn next_char(input: &str, index: usize) -> Option<char> {
    input[index..].chars().next()
}

fn is_word_char(ch: char) -> bool {
    ch.is_alphanumeric()
}

/// `_` and `_=` delimiters.
fn emphasis_marker(input: &str, index: usize) -> Option<(InlineToken<'_>, usize)> {
    let previous = prev_char(input, index);
    let next = next_char(input, index + 1);

    // `_=` closes `=_..._=`.
    if next == Some('=') {
        let after_equals = next_char(input, index + 2);
        let can_close = previous.is_some_and(|ch| !ch.is_whitespace())
            && !after_equals.is_some_and(is_word_char);
        if can_close {
            return Some((
                InlineToken::EmphasisMarker {
                    kind: EmphasisKind::BoldItalic,
                    marker: "_=",
                    can_open: false,
                    can_close: true,
                },
                2,
            ));
        }
    }

    let can_open =
        next.is_some_and(|ch| !ch.is_whitespace()) && !previous.is_some_and(is_word_char);
    let can_close =
        previous.is_some_and(|ch| !ch.is_whitespace()) && !next.is_some_and(is_word_char);

    if !can_open && !can_close {
        return None;
    }

    Some((
        InlineToken::EmphasisMarker {
            kind: EmphasisKind::Italic,
            marker: "_",
            can_open,
            can_close,
        },
        1,
    ))
}

/// `*` delimiters. A word character may sit on either side, matching the
/// upstream format convention; only whitespace is rejected.
fn bold_marker(input: &str, index: usize) -> Option<(InlineToken<'_>, usize)> {
    let previous = prev_char(input, index);
    let next = next_char(input, index + 1);

    let can_open = next.is_some_and(|ch| !ch.is_whitespace());
    let can_close = previous.is_some_and(|ch| !ch.is_whitespace());
    if !can_open && !can_close {
        return None;
    }

    Some((
        InlineToken::EmphasisMarker {
            kind: EmphasisKind::Bold,
            marker: "*",
            can_open,
            can_close,
        },
        1,
    ))
}

/// `=_` opens bold italic.
fn bold_italic_open(input: &str, index: usize) -> Option<(InlineToken<'_>, usize)> {
    if next_char(input, index + 1) != Some('_') {
        return None;
    }

    let previous = prev_char(input, index);
    let after_underscore = next_char(input, index + 2);
    let can_open = after_underscore.is_some_and(|ch| !ch.is_whitespace())
        && !previous.is_some_and(is_word_char);
    if !can_open {
        return None;
    }

    Some((
        InlineToken::EmphasisMarker {
            kind: EmphasisKind::BoldItalic,
            marker: "=_",
            can_open: true,
            can_close: false,
        },
        2,
    ))
}

/// `<sc>` / `</sc>` tags (case-insensitive).
fn small_caps(input: &str, index: usize) -> Option<(InlineToken<'_>, usize)> {
    let rest = &input[index..];
    if rest
        .get(..5)
        .is_some_and(|tag| tag.eq_ignore_ascii_case("</sc>"))
    {
        return Some((InlineToken::SmallCapsClose, 5));
    }
    if rest
        .get(..4)
        .is_some_and(|tag| tag.eq_ignore_ascii_case("<sc>"))
    {
        return Some((InlineToken::SmallCapsOpen, 4));
    }
    None
}

/// A complete `[Footnote N: text]` span. Malformed spans are not tokens.
fn footnote(input: &str, index: usize) -> Option<(InlineToken<'_>, usize)> {
    const PREFIX: &str = "[Footnote";

    let rest = &input[index..];
    if !rest
        .get(..PREFIX.len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(PREFIX))
    {
        return None;
    }

    let bytes = rest.as_bytes();
    let mut offset = PREFIX.len();

    while bytes.get(offset) == Some(&b' ') {
        offset += 1;
    }

    let digits_start = offset;
    while bytes.get(offset).is_some_and(u8::is_ascii_digit) {
        offset += 1;
    }
    if offset == digits_start {
        return None;
    }
    let number = rest[digits_start..offset].parse::<u32>().ok()?;

    if bytes.get(offset) != Some(&b':') {
        return None;
    }
    offset += 1;
    if bytes.get(offset) == Some(&b' ') {
        offset += 1;
    }

    let content_start = offset;
    let close = rest[content_start..].find(']')?;
    let content = &rest[content_start..content_start + close];
    let consumed = content_start + close + 1;

    Some((InlineToken::Footnote { number, content }, consumed))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_block_lines() {
        assert_eq!(classify_line(""), LineKind::Blank);
        assert_eq!(classify_line("   "), LineKind::Blank);
        assert_eq!(classify_line("----"), LineKind::HorizontalRule);
        assert_eq!(classify_line("-------"), LineKind::HorizontalRule);
        assert_eq!(classify_line("---"), LineKind::Paragraph("---"));
        assert_eq!(
            classify_line("# Trích dẫn"),
            LineKind::Blockquote("Trích dẫn")
        );
        assert_eq!(classify_line("#"), LineKind::Blockquote(""));
        assert_eq!(classify_line("/>  thơ"), LineKind::Poetry("  thơ"));
        assert_eq!(classify_line("Văn bản"), LineKind::Paragraph("Văn bản"));
    }

    #[test]
    fn tokenizes_emphasis_and_small_caps() {
        let tokens = tokenize_inline("_nghiêng_ *đậm* =_cả hai_= <sc>việt</sc>");
        assert!(tokens.iter().any(|token| matches!(
            token,
            InlineToken::EmphasisMarker {
                kind: EmphasisKind::Italic,
                ..
            }
        )));
        assert!(tokens.iter().any(|token| matches!(
            token,
            InlineToken::EmphasisMarker {
                kind: EmphasisKind::Bold,
                ..
            }
        )));
        assert!(tokens.iter().any(|token| matches!(
            token,
            InlineToken::EmphasisMarker {
                kind: EmphasisKind::BoldItalic,
                marker: "=_",
                ..
            }
        )));
        assert!(tokens.iter().any(|token| matches!(
            token,
            InlineToken::EmphasisMarker {
                kind: EmphasisKind::BoldItalic,
                marker: "_=",
                ..
            }
        )));
        assert!(
            tokens
                .iter()
                .any(|token| matches!(token, InlineToken::SmallCapsOpen))
        );
        assert!(
            tokens
                .iter()
                .any(|token| matches!(token, InlineToken::SmallCapsClose))
        );
    }

    #[test]
    fn underscore_inside_a_word_is_text() {
        let tokens = tokenize_inline("snake_case");
        assert!(
            tokens
                .iter()
                .all(|token| matches!(token, InlineToken::Text(_)))
        );
    }

    #[test]
    fn footnote_token_captures_number_and_content() {
        let tokens = tokenize_inline("Trước [Footnote 3: Ghi chú _quan trọng_] sau");
        let footnote = tokens
            .iter()
            .find_map(|token| match token {
                InlineToken::Footnote { number, content } => Some((*number, *content)),
                _ => None,
            })
            .expect("footnote token");
        assert_eq!(footnote, (3, "Ghi chú _quan trọng_"));
    }

    #[test]
    fn malformed_footnote_stays_text() {
        let tokens = tokenize_inline("[Footnote 1: thiếu đóng");
        assert!(
            tokens
                .iter()
                .all(|token| matches!(token, InlineToken::Text(_)))
        );
    }

    #[test]
    fn token_offsets_cover_input_without_loss() {
        let tokens = tokenize_inline("Tôi _nghỉ_ học <sc>và</sc> [Footnote 2: ghi chú].");
        let reconstructed: String = tokens
            .iter()
            .map(|token| match token {
                InlineToken::Text(text) => (*text).to_owned(),
                InlineToken::EmphasisMarker { marker, .. } => (*marker).to_owned(),
                InlineToken::SmallCapsOpen => "<sc>".to_owned(),
                InlineToken::SmallCapsClose => "</sc>".to_owned(),
                InlineToken::Footnote { number, content } => {
                    format!("[Footnote {number}: {content}]")
                }
            })
            .collect();
        assert_eq!(
            reconstructed,
            "Tôi _nghỉ_ học <sc>và</sc> [Footnote 2: ghi chú]."
        );
    }

    proptest::proptest! {
        // Text without tag or footnote openers has a lossless token stream.
        #[test]
        fn prop_tokens_are_lossless(s in "[^\\[<]{0,300}") {
            let reconstructed: String = tokenize_inline(&s)
                .iter()
                .map(|token| match token {
                    InlineToken::Text(text) => (*text).to_owned(),
                    InlineToken::EmphasisMarker { marker, .. } => (*marker).to_owned(),
                    InlineToken::SmallCapsOpen => "<sc>".to_owned(),
                    InlineToken::SmallCapsClose => "</sc>".to_owned(),
                    InlineToken::Footnote { number, content } => {
                        format!("[Footnote {number}: {content}]")
                    }
                })
                .collect();
            proptest::prop_assert_eq!(reconstructed, s);
        }
    }
}
