//! Safe Markdown rendering for project comments (§13.1, §7.2).
//!
//! Comments are authored as Markdown, rendered once at write time, and stored in
//! both source and rendered form. The rendered fragment is sanitized with
//! `ammonia` so it can be inserted into a template without further escaping.

use pulldown_cmark::{Options, Parser, html};

/// Render a Markdown comment to sanitized HTML.
///
/// The returned fragment is safe to embed verbatim: `ammonia` removes scripts,
/// event handlers, and other active content while allowing ordinary formatting
/// (`p`, `em`, `strong`, `a`, `ul`, `ol`, `li`, `code`, `pre`, `blockquote`,
/// headings, and tables).
///
/// # Examples
///
/// ```
/// use dp_core::markdown::render_markdown;
///
/// let html = render_markdown("**Chào** bạn");
/// assert!(html.contains("<strong>Chào</strong>"));
/// assert!(!render_markdown("<script>alert(1)</script>").contains("<script"));
/// ```
#[must_use]
pub fn render_markdown(markdown: &str) -> String {
    if markdown.trim().is_empty() {
        return String::new();
    }

    let mut options = Options::empty();
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_FOOTNOTES);

    let parser = Parser::new_ext(markdown, options);
    let mut rendered = String::with_capacity(markdown.len());
    html::push_html(&mut rendered, parser);

    ammonia::Builder::default().clean(&rendered).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_common_formatting() {
        let html = render_markdown("**đậm** và *nghiêng*");
        assert!(html.contains("<strong>đậm</strong>"));
        assert!(html.contains("<em>nghiêng</em>"));
    }

    #[test]
    fn strips_script_tags_and_event_handlers() {
        let html = render_markdown("<script>alert('x')</script>\n\n[x](javascript:alert(1))");
        assert!(!html.contains("<script"));
        assert!(!html.to_lowercase().contains("javascript:"));
        assert!(!html.contains("onerror"));
    }

    #[test]
    fn empty_input_is_empty() {
        assert!(render_markdown("").is_empty());
        assert!(render_markdown("   \n").is_empty());
    }

    #[test]
    fn preserves_vietnamese_text() {
        let html = render_markdown("Truyện Kiều — Nguyễn Du");
        assert!(html.contains("Truyện Kiều — Nguyễn Du"));
    }
}
