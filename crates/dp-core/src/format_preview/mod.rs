//! Format preview: a deliberately small, safe markup subset (§19 Phase 9).
//!
//! The accepted markup is locked in [`spec.md`](spec). The parser is split into
//! a lexer and a small AST; the renderer escapes every byte of page text and
//! passes the assembled fragment through `ammonia` as defense in depth.
//!
//! # Examples
//!
//! ```
//! use dp_core::format_preview::render_preview;
//!
//! let html = render_preview("Đoạn _in nghiêng_ và *in đậm*.");
//! assert!(html.contains("<i>in nghiêng</i>"));
//! assert!(html.contains("<b>in đậm</b>"));
//! ```

pub mod lexer;
pub mod parser;

use std::collections::HashSet;
use std::fmt::Write as _;
use std::sync::LazyLock;

use parser::{Block, EmphasisKind, Inline, PoetryLine};

/// Maximum number of nested inline frames. See [`lexer::MAX_NESTING_DEPTH`].
pub const MAX_NESTING_DEPTH: usize = lexer::MAX_NESTING_DEPTH;

/// Render page text to a safe HTML fragment for the preview pane.
///
/// The fragment contains only the allowlisted preview tags; it is safe to insert
/// into a template with an autoescape bypass.
#[must_use]
pub fn render_preview(text: &str) -> String {
    let blocks = parser::parse(text);
    if blocks.is_empty() {
        return String::new();
    }

    let mut renderer = PreviewRenderer::default();
    renderer.render_blocks(&blocks);
    let raw = renderer.finish();
    if raw.is_empty() {
        String::new()
    } else {
        SANITIZER.clean(&raw).to_string()
    }
}

/// Parse source text into the preview AST without rendering it.
#[must_use]
pub fn parse_preview(text: &str) -> Vec<Block> {
    parser::parse(text)
}

/// Explicit allowlist for the preview fragment.
static SANITIZER: LazyLock<ammonia::Builder<'static>> = LazyLock::new(|| {
    let mut builder = ammonia::Builder::default();
    builder
        .tags(HashSet::from([
            "a",
            "b",
            "blockquote",
            "br",
            "div",
            "hr",
            "i",
            "li",
            "ol",
            "p",
            "span",
            "sup",
        ]))
        .add_generic_attributes(["class", "id", "aria-label"])
        .add_tag_attributes("a", ["href", "class"])
        .add_tag_attributes("li", ["id", "value"])
        .add_tag_attributes("ol", ["class"])
        .add_tag_attributes("span", ["class"])
        .add_tag_attributes("sup", ["class", "id"]);
    builder
});

#[derive(Default)]
struct PreviewRenderer {
    html: String,
    footnotes: Vec<(u32, Vec<Inline>)>,
    footnote_numbers: HashSet<u32>,
}

impl PreviewRenderer {
    fn render_blocks(&mut self, blocks: &[Block]) {
        for block in blocks {
            match block {
                Block::Paragraph(inlines) => {
                    self.html.push_str("<p>");
                    self.render_inlines(inlines, false);
                    self.html.push_str("</p>");
                }
                Block::Blockquote(paragraphs) => {
                    self.html.push_str("<blockquote>");
                    for paragraph in paragraphs {
                        self.html.push_str("<p>");
                        self.render_inlines(paragraph, false);
                        self.html.push_str("</p>");
                    }
                    self.html.push_str("</blockquote>");
                }
                Block::Poetry(lines) => self.render_poetry(lines),
                Block::HorizontalRule => self.html.push_str("<hr>"),
            }
        }
    }

    fn render_poetry(&mut self, lines: &[PoetryLine]) {
        self.html.push_str("<div class=\"poetry\">");
        for line in lines {
            self.html.push_str("<div class=\"poem-line\">");
            self.render_inlines(&line.content, false);
            self.html.push_str("</div>");
        }
        self.html.push_str("</div>");
    }

    fn render_inlines(&mut self, inlines: &[Inline], uppercase: bool) {
        for inline in inlines {
            match inline {
                Inline::Text(text) => {
                    if uppercase {
                        let upper = text.to_uppercase();
                        escape_into(&mut self.html, &upper);
                    } else {
                        escape_into(&mut self.html, text);
                    }
                }
                Inline::Emphasis { kind, children } => match kind {
                    EmphasisKind::Italic => {
                        self.html.push_str("<i>");
                        self.render_inlines(children, uppercase);
                        self.html.push_str("</i>");
                    }
                    EmphasisKind::Bold => {
                        self.html.push_str("<b>");
                        self.render_inlines(children, uppercase);
                        self.html.push_str("</b>");
                    }
                    EmphasisKind::BoldItalic => {
                        self.html.push_str("<b><i>");
                        self.render_inlines(children, uppercase);
                        self.html.push_str("</i></b>");
                    }
                },
                Inline::SmallCaps(children) => {
                    self.html.push_str("<span class=\"sc\">");
                    self.render_inlines(children, true);
                    self.html.push_str("</span>");
                }
                Inline::FootnoteRef { number, text } => {
                    let first_reference = self.footnote_numbers.insert(*number);
                    let _ = write!(
                        self.html,
                        "<sup class=\"fnref\"{}><a href=\"#fn-{number}\">{number}</a></sup>",
                        if first_reference {
                            format!(" id=\"fnref-{number}\"")
                        } else {
                            String::new()
                        }
                    );
                    if first_reference {
                        self.footnotes.push((*number, text.clone()));
                    }
                }
            }
        }
    }

    fn finish(mut self) -> String {
        if !self.footnotes.is_empty() {
            self.html.push_str("<ol class=\"footnotes\">");
            let mut index = 0;
            while index < self.footnotes.len() {
                let (number, text) = self.footnotes[index].clone();
                let _ = write!(
                    self.html,
                    "<li id=\"fn-{number}\" value=\"{number}\"><span class=\"fn-text\">"
                );
                self.render_inlines(&text, false);
                let _ = write!(
                    self.html,
                    "</span> <a class=\"fn-back\" href=\"#fnref-{number}\" aria-label=\"Quay lại ghi chú\">&#8617;</a></li>"
                );
                index += 1;
            }
            self.html.push_str("</ol>");
        }
        self.html
    }
}

fn escape_into(out: &mut String, text: &str) {
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#x27;"),
            _ => out.push(ch),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn renders_paragraphs_and_blank_lines() {
        let html = render_preview("Đoạn một.\n\nĐoạn hai.");
        assert_eq!(html, "<p>Đoạn một.</p><p>Đoạn hai.</p>");
    }

    #[test]
    fn renders_emphasis_variants() {
        let html = render_preview("_nghiêng_ *đậm* =_cả hai_=");
        assert!(html.contains("<i>nghiêng</i>"), "html: {html}");
        assert!(html.contains("<b>đậm</b>"), "html: {html}");
        assert!(html.contains("<b><i>cả hai</i></b>"), "html: {html}");
    }

    #[test]
    fn renders_small_caps_uppercased() {
        let html = render_preview("<sc>Việt Nam</sc>");
        assert!(
            html.contains("<span class=\"sc\">VIỆT NAM</span>"),
            "html: {html}"
        );
    }

    #[test]
    fn renders_blockquote_poetry_and_rule() {
        let html = render_preview("# Trích dẫn\n/>   thơ\n----");
        assert!(
            html.contains("<blockquote><p>Trích dẫn</p></blockquote>"),
            "html: {html}"
        );
        assert!(
            html.contains("<div class=\"poetry\"><div class=\"poem-line\">   thơ</div></div>"),
            "html: {html}"
        );
        assert!(html.contains("<hr>"), "html: {html}");
    }

    #[test]
    fn renders_footnote_reference_and_list() {
        let html = render_preview("Câu văn.[Footnote 1: Ghi chú _quan trọng_]");
        assert!(
            html.contains("<sup class=\"fnref\" id=\"fnref-1\">"),
            "html: {html}"
        );
        assert!(html.contains("href=\"#fn-1\""), "html: {html}");
        assert!(html.contains("<ol class=\"footnotes\">"), "html: {html}");
        assert!(
            html.contains("<li id=\"fn-1\" value=\"1\">"),
            "html: {html}"
        );
        assert!(html.contains("<i>quan trọng</i>"), "html: {html}");
    }

    #[test]
    fn unclosed_markers_render_literally() {
        let html = render_preview("_nghiêng *đậm <sc>nhỏ");
        assert!(!html.contains("<i>"), "html: {html}");
        assert!(!html.contains("<b>"), "html: {html}");
        assert!(!html.contains("class=\"sc\""), "html: {html}");
        assert!(html.contains("_nghiêng *đậm &lt;sc&gt;nhỏ"), "html: {html}");
    }

    #[test]
    fn snake_case_survives() {
        let html = render_preview("snake_case");
        assert!(html.contains("snake_case"), "html: {html}");
        assert!(!html.contains("<i>"), "html: {html}");
    }

    #[test]
    fn page_text_cannot_inject_html() {
        let html = render_preview("<script>alert(1)</script><img src=x onerror=alert(2)>");
        assert!(!html.contains("<script"), "html: {html}");
        assert!(!html.contains("<img"), "html: {html}");
        assert!(html.contains("&lt;script&gt;"), "html: {html}");
    }

    #[test]
    fn malformed_footnote_is_literal() {
        let html = render_preview("[Footnote 1: thiếu đóng");
        assert!(html.contains("[Footnote 1: thiếu đóng"), "html: {html}");
        assert!(!html.contains("<ol class=\"footnotes\">"), "html: {html}");
    }

    #[test]
    fn vietnamese_diacritics_are_preserved() {
        let html = render_preview("Truyện Kiều — Nguyễn Du");
        assert!(html.contains("Truyện Kiều — Nguyễn Du"), "html: {html}");
    }

    #[test]
    fn deep_nesting_does_not_blow_the_stack() {
        let text = format!("{}x{}", "_".repeat(500), "_".repeat(500));
        let html = render_preview(&text);
        assert!(html.contains('x'), "html: {html}");
    }

    #[test]
    fn preview_is_fast_enough_for_a_page() {
        let text = "Trăm năm trong cõi người ta, chữ tài chữ mệnh khéo là ghét nhau. \
                    _Đoạn in nghiêng_ và *đoạn in đậm* — Truyện Kiều.\n\n"
            .repeat(30);
        assert!(text.chars().count() >= 3_000);

        let mut best = std::time::Duration::MAX;
        let mut html = String::new();
        for _ in 0..3 {
            let start = std::time::Instant::now();
            html = render_preview(&text);
            best = best.min(start.elapsed());
        }

        assert!(!html.is_empty());
        assert!(
            best < std::time::Duration::from_millis(5),
            "3k-character preview took {best:?}"
        );
    }

    proptest! {
        #[test]
        fn prop_render_never_panics_and_never_emits_scripts(s in "\\PC{0,400}") {
            let html = render_preview(&s);
            prop_assert!(!html.to_lowercase().contains("<script"));
            prop_assert!(!html.to_lowercase().contains("<img"));
        }

        #[test]
        fn prop_render_is_deterministic(s in "\\PC{0,400}") {
            prop_assert_eq!(render_preview(&s), render_preview(&s));
        }
    }
}
