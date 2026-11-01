//! Parser producing a small AST for the format-preview markup subset.
//!
//! Block structure comes from [`classify_line`]; inline structure is balanced
//! with a bounded stack so pathological input can never recurse or grow without
//! limit. Anything left unclosed is converted back to literal text.

use super::lexer::{InlineToken, LineKind, MAX_NESTING_DEPTH, classify_line, tokenize_inline};

pub use super::lexer::EmphasisKind;

/// Inline content inside a paragraph, blockquote, or footnote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Inline {
    /// Literal text.
    Text(String),
    /// `_…_`, `*…*`, or `=_…_=`.
    Emphasis {
        /// Which emphasis variant was opened.
        kind: EmphasisKind,
        /// Nested inline content.
        children: Vec<Inline>,
    },
    /// `<sc>…</sc>`.
    SmallCaps(Vec<Inline>),
    /// A reference to a footnote whose body is rendered at the end.
    FootnoteRef {
        /// Number as written in the source.
        number: u32,
        /// Parsed footnote body.
        text: Vec<Inline>,
    },
}

/// One line inside a poetry block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PoetryLine {
    /// Inline content of the line, excluding the leading `/>` marker.
    pub content: Vec<Inline>,
}

/// A top-level block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    /// A run of ordinary lines.
    Paragraph(Vec<Inline>),
    /// One or more `#` lines; each inner vector is one paragraph.
    Blockquote(Vec<Vec<Inline>>),
    /// One or more `/>` lines.
    Poetry(Vec<PoetryLine>),
    /// `----`.
    HorizontalRule,
}

/// Parse source text into blocks.
///
/// # Examples
///
/// ```
/// use dp_core::format_preview::parser::{Block, parse};
///
/// let blocks = parse("Đoạn một\n\n_Đoạn hai_");
/// assert_eq!(blocks.len(), 2);
/// assert!(matches!(blocks[0], Block::Paragraph(_)));
/// ```
#[must_use]
pub fn parse(text: &str) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut paragraph: Vec<&str> = Vec::new();
    let mut quote: Vec<Vec<Inline>> = Vec::new();
    let mut poetry: Vec<PoetryLine> = Vec::new();

    for line in text.split('\n') {
        match classify_line(line) {
            LineKind::Blank => {
                flush_paragraph(&mut blocks, &mut paragraph);
                flush_quote(&mut blocks, &mut quote);
                flush_poetry(&mut blocks, &mut poetry);
            }
            LineKind::HorizontalRule => {
                flush_paragraph(&mut blocks, &mut paragraph);
                flush_quote(&mut blocks, &mut quote);
                flush_poetry(&mut blocks, &mut poetry);
                blocks.push(Block::HorizontalRule);
            }
            LineKind::Blockquote(content) => {
                flush_paragraph(&mut blocks, &mut paragraph);
                flush_poetry(&mut blocks, &mut poetry);
                quote.push(parse_inlines(content, 0));
            }
            LineKind::Poetry(content) => {
                flush_paragraph(&mut blocks, &mut paragraph);
                flush_quote(&mut blocks, &mut quote);
                poetry.push(PoetryLine {
                    content: parse_inlines(content, 0),
                });
            }
            LineKind::Paragraph(content) => {
                flush_quote(&mut blocks, &mut quote);
                flush_poetry(&mut blocks, &mut poetry);
                paragraph.push(content);
            }
        }
    }

    flush_paragraph(&mut blocks, &mut paragraph);
    flush_quote(&mut blocks, &mut quote);
    flush_poetry(&mut blocks, &mut poetry);

    blocks
}

fn flush_paragraph(blocks: &mut Vec<Block>, paragraph: &mut Vec<&str>) {
    if paragraph.is_empty() {
        return;
    }
    let joined = paragraph.join("\n");
    paragraph.clear();

    let inlines = parse_inlines(&joined, 0);
    if !inlines.is_empty() {
        blocks.push(Block::Paragraph(inlines));
    }
}

fn flush_quote(blocks: &mut Vec<Block>, quote: &mut Vec<Vec<Inline>>) {
    if quote.is_empty() {
        return;
    }
    blocks.push(Block::Blockquote(std::mem::take(quote)));
}

fn flush_poetry(blocks: &mut Vec<Block>, poetry: &mut Vec<PoetryLine>) {
    if poetry.is_empty() {
        return;
    }
    blocks.push(Block::Poetry(std::mem::take(poetry)));
}

/// The kind of an open stack frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OpenKind {
    Emphasis(EmphasisKind),
    SmallCaps,
}

struct Frame<'a> {
    kind: Option<OpenKind>,
    children: Vec<Inline>,
    marker: &'a str,
}

/// Parse inline markup, capping nesting at [`MAX_NESTING_DEPTH`].
fn parse_inlines(text: &str, depth: usize) -> Vec<Inline> {
    if depth >= MAX_NESTING_DEPTH {
        return if text.is_empty() {
            Vec::new()
        } else {
            vec![Inline::Text(text.to_owned())]
        };
    }

    let mut stack = vec![Frame {
        kind: None,
        children: Vec::new(),
        marker: "",
    }];

    for token in tokenize_inline(text) {
        match token {
            InlineToken::Text(text) => push_text(&mut stack, text),
            InlineToken::Footnote { number, content } => {
                let body = parse_inlines(content, depth + 1);
                current(&mut stack)
                    .children
                    .push(Inline::FootnoteRef { number, text: body });
            }
            InlineToken::SmallCapsOpen => push_open(&mut stack, OpenKind::SmallCaps, "<sc>", depth),
            InlineToken::SmallCapsClose => {
                if stack
                    .last()
                    .is_some_and(|frame| frame.kind == Some(OpenKind::SmallCaps))
                {
                    close_top(&mut stack);
                } else {
                    push_text(&mut stack, "</sc>");
                }
            }
            InlineToken::EmphasisMarker {
                kind,
                marker,
                can_open,
                can_close,
            } => {
                let wanted = OpenKind::Emphasis(kind);
                let top_matches = stack.last().is_some_and(|frame| frame.kind == Some(wanted));

                if can_close && top_matches {
                    close_top(&mut stack);
                } else if can_open {
                    push_open(&mut stack, wanted, marker, depth);
                } else {
                    push_text(&mut stack, marker);
                }
            }
        }
    }

    unwind(stack)
}

fn current<'a, 'b>(stack: &'b mut [Frame<'a>]) -> &'b mut Frame<'a> {
    stack.last_mut().expect("the base frame is always present")
}

fn push_text(stack: &mut [Frame<'_>], text: &str) {
    if text.is_empty() {
        return;
    }
    let children = &mut current(stack).children;
    if let Some(Inline::Text(previous)) = children.last_mut() {
        previous.push_str(text);
    } else {
        children.push(Inline::Text(text.to_owned()));
    }
}

fn push_open<'a>(stack: &mut Vec<Frame<'a>>, kind: OpenKind, marker: &'a str, depth: usize) {
    // Frame 0 is the synthetic root, so `stack.len() - 1` is the open depth.
    if stack.len() <= MAX_NESTING_DEPTH.saturating_sub(depth) {
        stack.push(Frame {
            kind: Some(kind),
            children: Vec::new(),
            marker,
        });
    } else {
        push_text(stack, marker);
    }
}

fn close_top(stack: &mut Vec<Frame<'_>>) {
    let Some(frame) = stack.pop() else {
        return;
    };
    let Some(kind) = frame.kind else {
        stack.push(frame);
        return;
    };

    let node = match kind {
        OpenKind::Emphasis(kind) => Inline::Emphasis {
            kind,
            children: frame.children,
        },
        OpenKind::SmallCaps => Inline::SmallCaps(frame.children),
    };
    current(stack).children.push(node);
}

/// Convert every unclosed frame back into literal marker text.
fn unwind(mut stack: Vec<Frame<'_>>) -> Vec<Inline> {
    while stack.len() > 1 {
        let Some(frame) = stack.pop() else {
            break;
        };
        let Some(kind) = frame.kind else {
            current(&mut stack).children.extend(frame.children);
            continue;
        };

        let marker = match kind {
            OpenKind::SmallCaps => "<sc>",
            OpenKind::Emphasis(_) => frame.marker,
        };

        let mut literal = Vec::with_capacity(frame.children.len() + 1);
        literal.push(Inline::Text(marker.to_owned()));
        literal.extend(frame.children);
        current(&mut stack).children.extend(literal);
    }

    stack.pop().map(|frame| frame.children).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain_text(inlines: &[Inline]) -> String {
        inlines
            .iter()
            .map(|inline| match inline {
                Inline::Text(text) => text.clone(),
                Inline::Emphasis { children, .. } => plain_text(children),
                Inline::SmallCaps(children) => plain_text(children),
                Inline::FootnoteRef { text, .. } => plain_text(text),
            })
            .collect()
    }

    #[test]
    fn parses_blocks_and_inline_nesting() {
        let blocks = parse("Đoạn một\n\n_Đoạn *hai*_\n\n# Trích\n\n/>  thơ\n\n----");
        assert_eq!(blocks.len(), 5);

        match &blocks[0] {
            Block::Paragraph(inlines) => assert_eq!(plain_text(inlines), "Đoạn một"),
            other => panic!("expected paragraph, got {other:?}"),
        }

        match &blocks[1] {
            Block::Paragraph(inlines) => {
                assert!(matches!(
                    inlines.first(),
                    Some(Inline::Emphasis {
                        kind: EmphasisKind::Italic,
                        ..
                    })
                ));
            }
            other => panic!("expected paragraph, got {other:?}"),
        }

        assert!(matches!(&blocks[2], Block::Blockquote(paragraphs) if paragraphs.len() == 1));
        assert!(matches!(&blocks[3], Block::Poetry(lines) if lines.len() == 1));
        assert_eq!(blocks[4], Block::HorizontalRule);
    }

    #[test]
    fn horizontal_rule_is_a_block() {
        let blocks = parse("Trước\n----\nSau");
        assert_eq!(blocks.len(), 3);
        assert_eq!(blocks[1], Block::HorizontalRule);
    }

    #[test]
    fn unclosed_markers_become_literal_text() {
        let blocks = parse("_nghiêng *đậm");
        let Block::Paragraph(inlines) = &blocks[0] else {
            panic!("expected paragraph");
        };
        assert_eq!(plain_text(inlines), "_nghiêng *đậm");
        assert!(
            !inlines
                .iter()
                .any(|inline| matches!(inline, Inline::Emphasis { .. }))
        );
    }

    #[test]
    fn snake_case_is_not_emphasis() {
        let blocks = parse("snake_case và _thật_");
        let Block::Paragraph(inlines) = &blocks[0] else {
            panic!("expected paragraph");
        };
        assert_eq!(plain_text(inlines), "snake_case và thật");
        assert!(
            inlines
                .iter()
                .any(|inline| matches!(inline, Inline::Emphasis { .. }))
        );
    }

    #[test]
    fn bold_italic_uses_the_combined_pair() {
        let blocks = parse("=_cả hai_=");
        let Block::Paragraph(inlines) = &blocks[0] else {
            panic!("expected paragraph");
        };
        assert!(matches!(
            &inlines[0],
            Inline::Emphasis {
                kind: EmphasisKind::BoldItalic,
                ..
            }
        ));
    }

    #[test]
    fn deep_nesting_is_capped() {
        let mut text = String::new();
        for _ in 0..100 {
            text.push('_');
        }
        text.push('x');
        for _ in 0..100 {
            text.push('_');
        }

        // Must terminate and keep the text.
        let blocks = parse(&text);
        let Block::Paragraph(inlines) = &blocks[0] else {
            panic!("expected paragraph");
        };
        assert!(plain_text(inlines).contains('x'));
    }
}
