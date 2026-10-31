//! Word- and line-level text diffing with escaped, sanitized HTML output.
//!
//! Implements Section 10 of the implementation plan:
//! - Section 10.2: `similar`-based Myers diff at word level.
//! - Section 10.3: Two-column line context, `<ins>`/`<del>` markup, escaped text,
//!   visible-whitespace mode, and an optional changes-only projection.
//!
//! The rendered fragments are safe to insert into a template with an autoescape
//! bypass: every byte of user text is HTML-escaped before markup is added, and
//! the resulting fragment is passed through `ammonia` as defense in depth.

use std::collections::HashSet;

use similar::{Algorithm, ChangeTag, DiffOp, TextDiff};

/// Rendering options shared by the diff helpers.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DiffOptions {
    /// Render spaces, tabs, and newlines with visible markers (`·`, `→`, `↵`).
    pub show_whitespace: bool,
    /// Keep only changed rows in [`diff_rows`]; ignored by [`diff_html`].
    pub changes_only: bool,
}

/// The kind of an aligned two-column diff row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffRowKind {
    /// Both sides have the same line.
    Equal,
    /// The line exists only in the old text.
    Delete,
    /// The line exists only in the new text.
    Insert,
    /// The line was changed; word-level markers appear within both sides.
    Replace,
}

impl DiffRowKind {
    /// Stable lowercase identifier used by templates and CSS classes.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Equal => "equal",
            Self::Delete => "delete",
            Self::Insert => "insert",
            Self::Replace => "replace",
        }
    }
}

/// One aligned row in a two-column diff.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffRow {
    /// Whether this row is equal, deleted, inserted, or replaced.
    pub kind: DiffRowKind,
    /// 1-based old line number, when the row has an old side.
    pub old_line_number: Option<usize>,
    /// 1-based new line number, when the row has a new side.
    pub new_line_number: Option<usize>,
    /// Escaped and sanitized HTML for the old column.
    pub old_html: String,
    /// Escaped and sanitized HTML for the new column.
    pub new_html: String,
}

/// Render a word-level diff with `<del>`/`<ins>` markup.
///
/// Text is HTML-escaped before markup is added, so page content cannot inject
/// tags. The result is sanitized with `ammonia` and is safe to render verbatim.
///
/// # Examples
///
/// ```
/// use dp_core::diff::diff_html;
///
/// let html = diff_html("Tôi nghỉ học", "Tôi nghĩ học");
/// assert!(html.contains("<del"));
/// assert!(html.contains("<ins"));
/// ```
#[must_use]
pub fn diff_html(old: &str, new: &str) -> String {
    diff_html_with(old, new, DiffOptions::default())
}

/// Render a word-level diff with explicit whitespace rendering options.
#[must_use]
pub fn diff_html_with(old: &str, new: &str, options: DiffOptions) -> String {
    let sanitizer = Sanitizer::new();
    let diff = TextDiff::configure()
        .algorithm(Algorithm::Myers)
        .diff_words(old, new);

    let mut out = String::with_capacity(old.len() + new.len() + 32);
    for change in diff.iter_all_changes() {
        let escaped = escape_fragment(change.value(), options);
        match change.tag() {
            ChangeTag::Equal => out.push_str(&escaped),
            ChangeTag::Delete => {
                out.push_str("<del aria-label=\"đã xoá\">");
                out.push_str(&escaped);
                out.push_str("</del>");
            }
            ChangeTag::Insert => {
                out.push_str("<ins aria-label=\"đã thêm\">");
                out.push_str(&escaped);
                out.push_str("</ins>");
            }
        }
    }

    sanitizer.clean(&out)
}

/// Build aligned two-column rows with line numbers and word-level change markers.
///
/// Equal lines are paired; a replaced block is paired line by line, with
/// word-level `<del>`/`<ins>` markers inside each pair. Unpaired lines become
/// pure deletions or insertions.
///
/// # Examples
///
/// ```
/// use dp_core::diff::{DiffOptions, diff_rows};
///
/// let rows = diff_rows("Trang một\nTrang hai", "Trang một\nTrang ba", DiffOptions::default());
/// assert_eq!(rows.len(), 2);
/// assert!(rows[1].old_html.contains("<del"));
/// assert!(rows[1].new_html.contains("<ins"));
/// ```
#[must_use]
pub fn diff_rows(old: &str, new: &str, options: DiffOptions) -> Vec<DiffRow> {
    let sanitizer = Sanitizer::new();
    let old_lines = split_lines(old);
    let new_lines = split_lines(new);
    let diff = TextDiff::configure()
        .algorithm(Algorithm::Myers)
        .diff_slices::<str>(&old_lines, &new_lines);

    let mut rows = Vec::new();
    let ops = diff.ops();
    let mut index = 0;

    while index < ops.len() {
        match ops[index] {
            DiffOp::Equal {
                old_index,
                new_index,
                len,
            } => {
                for offset in 0..len {
                    let old_line = old_lines[old_index + offset];
                    let new_line = new_lines[new_index + offset];
                    rows.push(DiffRow {
                        kind: DiffRowKind::Equal,
                        old_line_number: Some(old_index + offset + 1),
                        new_line_number: Some(new_index + offset + 1),
                        old_html: sanitizer.clean(&escape_fragment(old_line, options)),
                        new_html: sanitizer.clean(&escape_fragment(new_line, options)),
                    });
                }
            }
            DiffOp::Delete {
                old_index, old_len, ..
            } => {
                // A delete followed by an insert is a replacement block. Pair the
                // lines so word-level markers can be rendered inside both columns.
                if let Some(DiffOp::Insert {
                    new_index: insert_index,
                    new_len,
                    ..
                }) = ops.get(index + 1)
                {
                    push_replacement_rows(
                        &mut rows,
                        ReplacementBlock {
                            old_lines: &old_lines,
                            old_start: old_index,
                            old_len,
                            new_lines: &new_lines,
                            new_start: *insert_index,
                            new_len: *new_len,
                        },
                        options,
                        &sanitizer,
                    );
                    index += 1;
                } else {
                    push_deleted_rows(
                        &mut rows, &old_lines, old_index, old_len, options, &sanitizer,
                    );
                }
            }
            DiffOp::Insert {
                old_index,
                new_index,
                new_len,
            } => {
                push_inserted_rows(
                    &mut rows, &new_lines, old_index, new_index, new_len, options, &sanitizer,
                );
            }
            DiffOp::Replace {
                old_index,
                old_len,
                new_index,
                new_len,
            } => {
                push_replacement_rows(
                    &mut rows,
                    ReplacementBlock {
                        old_lines: &old_lines,
                        old_start: old_index,
                        old_len,
                        new_lines: &new_lines,
                        new_start: new_index,
                        new_len,
                    },
                    options,
                    &sanitizer,
                );
            }
        }
        index += 1;
    }

    if options.changes_only {
        rows.retain(|row| row.kind != DiffRowKind::Equal);
    }

    rows
}

/// A paired delete/insert (or explicit replace) block of lines.
struct ReplacementBlock<'lines, 'text> {
    old_lines: &'lines [&'text str],
    old_start: usize,
    old_len: usize,
    new_lines: &'lines [&'text str],
    new_start: usize,
    new_len: usize,
}

fn push_replacement_rows(
    rows: &mut Vec<DiffRow>,
    block: ReplacementBlock<'_, '_>,
    options: DiffOptions,
    sanitizer: &Sanitizer,
) {
    let paired_len = block.old_len.max(block.new_len);
    for offset in 0..paired_len {
        let old_line = (offset < block.old_len).then(|| block.old_lines[block.old_start + offset]);
        let new_line = (offset < block.new_len).then(|| block.new_lines[block.new_start + offset]);

        match (old_line, new_line) {
            (Some(old_text), Some(new_text)) => {
                let mut old_html = String::with_capacity(old_text.len() + 32);
                let mut new_html = String::with_capacity(new_text.len() + 32);
                write_word_diff(
                    old_text,
                    new_text,
                    options,
                    &mut old_html,
                    &mut new_html,
                    sanitizer,
                );
                rows.push(DiffRow {
                    kind: DiffRowKind::Replace,
                    old_line_number: Some(block.old_start + offset + 1),
                    new_line_number: Some(block.new_start + offset + 1),
                    old_html,
                    new_html,
                });
            }
            (Some(old_text), None) => rows.push(DiffRow {
                kind: DiffRowKind::Delete,
                old_line_number: Some(block.old_start + offset + 1),
                new_line_number: None,
                old_html: wrap_line("del", "đã xoá", old_text, options, sanitizer),
                new_html: String::new(),
            }),
            (None, Some(new_text)) => rows.push(DiffRow {
                kind: DiffRowKind::Insert,
                old_line_number: None,
                new_line_number: Some(block.new_start + offset + 1),
                old_html: String::new(),
                new_html: wrap_line("ins", "đã thêm", new_text, options, sanitizer),
            }),
            (None, None) => {}
        }
    }
}

fn push_deleted_rows(
    rows: &mut Vec<DiffRow>,
    old_lines: &[&str],
    old_start: usize,
    old_len: usize,
    options: DiffOptions,
    sanitizer: &Sanitizer,
) {
    for offset in 0..old_len {
        let text = old_lines[old_start + offset];
        rows.push(DiffRow {
            kind: DiffRowKind::Delete,
            old_line_number: Some(old_start + offset + 1),
            new_line_number: None,
            old_html: wrap_line("del", "đã xoá", text, options, sanitizer),
            new_html: String::new(),
        });
    }
}

fn push_inserted_rows(
    rows: &mut Vec<DiffRow>,
    new_lines: &[&str],
    _old_index: usize,
    new_start: usize,
    new_len: usize,
    options: DiffOptions,
    sanitizer: &Sanitizer,
) {
    for offset in 0..new_len {
        let text = new_lines[new_start + offset];
        rows.push(DiffRow {
            kind: DiffRowKind::Insert,
            old_line_number: None,
            new_line_number: Some(new_start + offset + 1),
            old_html: String::new(),
            new_html: wrap_line("ins", "đã thêm", text, options, sanitizer),
        });
    }
}

/// Write old/new word-level fragments into separate output buffers.
fn write_word_diff(
    old: &str,
    new: &str,
    options: DiffOptions,
    old_out: &mut String,
    new_out: &mut String,
    sanitizer: &Sanitizer,
) {
    let diff = TextDiff::configure()
        .algorithm(Algorithm::Myers)
        .diff_words(old, new);

    for change in diff.iter_all_changes() {
        match change.tag() {
            ChangeTag::Equal => {
                let escaped = escape_fragment(change.value(), options);
                old_out.push_str(&escaped);
                new_out.push_str(&escaped);
            }
            ChangeTag::Delete => {
                let inner = escape_fragment(change.value(), options);
                old_out.push_str("<del aria-label=\"đã xoá\">");
                old_out.push_str(&inner);
                old_out.push_str("</del>");
            }
            ChangeTag::Insert => {
                let inner = escape_fragment(change.value(), options);
                new_out.push_str("<ins aria-label=\"đã thêm\">");
                new_out.push_str(&inner);
                new_out.push_str("</ins>");
            }
        }
    }

    sanitize_in_place(old_out, sanitizer);
    sanitize_in_place(new_out, sanitizer);
}

fn sanitize_in_place(html: &mut String, sanitizer: &Sanitizer) {
    if html.is_empty() {
        return;
    }
    let cleaned = sanitizer.clean(html);
    html.clear();
    html.push_str(&cleaned);
}

fn wrap_line(
    tag: &str,
    label: &str,
    text: &str,
    options: DiffOptions,
    sanitizer: &Sanitizer,
) -> String {
    let mut html = String::with_capacity(text.len() + 32);
    html.push('<');
    html.push_str(tag);
    html.push_str(" aria-label=\"");
    html.push_str(label);
    html.push_str("\">");
    let escaped = escape_fragment(text, options);
    html.push_str(&escaped);
    html.push_str("</");
    html.push_str(tag);
    html.push('>');
    sanitizer.clean(&html)
}

/// Split text into lines without collapsing a trailing empty line.
fn split_lines(text: &str) -> Vec<&str> {
    if text.is_empty() {
        Vec::new()
    } else {
        text.split('\n').collect()
    }
}

/// HTML-escape a text fragment and optionally make whitespace visible.
fn escape_fragment(text: &str, options: DiffOptions) -> String {
    let mut out = String::with_capacity(text.len() + text.len() / 8);
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#x27;"),
            ' ' if options.show_whitespace => out.push_str("<span class=\"ws\">·</span>"),
            '\t' if options.show_whitespace => out.push_str("<span class=\"ws\">→</span>"),
            '\n' if options.show_whitespace => out.push_str("<span class=\"ws\">↵</span>\n"),
            _ => out.push(ch),
        }
    }
    out
}

/// Small wrapper around a configured `ammonia` builder.
struct Sanitizer {
    builder: ammonia::Builder<'static>,
}

impl Sanitizer {
    fn new() -> Self {
        let mut builder = ammonia::Builder::default();
        builder
            .tags(HashSet::from(["del", "ins", "span"]))
            .add_generic_attributes(["aria-label", "class"])
            .add_tag_attributes("del", ["aria-label"])
            .add_tag_attributes("ins", ["aria-label"])
            .add_tag_attributes("span", ["class"]);
        Self { builder }
    }

    fn clean(&self, html: &str) -> String {
        if html.is_empty() {
            return String::new();
        }
        self.builder.clean(html).to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diff_html_escapes_page_text() {
        let html = diff_html(
            "<script>alert('old')</script>",
            "<script>alert('new')</script>",
        );
        assert!(!html.contains("<script"), "unescaped script tag: {html}");
        assert!(
            html.contains("&lt;script&gt;"),
            "escaped script missing: {html}"
        );
        assert!(html.contains("<del"), "delete marker missing: {html}");
        assert!(html.contains("<ins"), "insert marker missing: {html}");
    }

    #[test]
    fn diff_rows_escape_page_text() {
        let rows = diff_rows(
            "<b>old</b>\nline 2",
            "<img src=x>\nline 2",
            DiffOptions::default(),
        );
        assert_eq!(rows.len(), 2);
        assert!(!rows[0].old_html.contains("<b>"));
        assert!(!rows[0].new_html.contains("<img"));
        assert!(rows[0].old_html.contains("&lt;b&gt;"));
        assert!(rows[0].new_html.contains("&lt;img"));
    }

    #[test]
    fn changes_only_removes_equal_rows() {
        let rows = diff_rows(
            "giữ nguyên\nthay đổi",
            "giữ nguyên\nđã đổi",
            DiffOptions {
                changes_only: true,
                ..DiffOptions::default()
            },
        );
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].kind, DiffRowKind::Replace);
        assert_eq!(rows[0].old_line_number, Some(2));
    }

    #[test]
    fn whitespace_toggle_marks_spaces() {
        let rows = diff_rows(
            "một hai",
            "một  hai",
            DiffOptions {
                show_whitespace: true,
                ..DiffOptions::default()
            },
        );
        assert!(rows.iter().any(|row| row.old_html.contains("class=\"ws\"")));
        assert!(rows.iter().any(|row| row.new_html.contains("class=\"ws\"")));
    }

    #[test]
    fn empty_texts_produce_no_rows() {
        assert!(diff_rows("", "", DiffOptions::default()).is_empty());
    }

    #[test]
    fn identical_text_is_unchanged() {
        let rows = diff_rows(
            "Trang một\nTrang hai",
            "Trang một\nTrang hai",
            DiffOptions::default(),
        );
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|row| row.kind == DiffRowKind::Equal));
    }

    #[test]
    fn word_level_vietnamese_diacritics() {
        let rows = diff_rows("Tôi nghỉ học", "Tôi nghĩ học", DiffOptions::default());
        assert_eq!(rows.len(), 1);
        assert!(rows[0].old_html.contains("<del"));
        assert!(rows[0].new_html.contains("<ins"));
        assert!(rows[0].old_html.contains("nghỉ"));
        assert!(rows[0].new_html.contains("nghĩ"));
    }

    #[test]
    fn diff_html_is_fast_enough_for_a_page() {
        let old = "Trăm năm trong cõi người ta, chữ tài chữ mệnh khéo là ghét nhau. ".repeat(50);
        let new = old.replace("ghét", "thương");
        assert!(old.chars().count() >= 3_000);

        // Warm up once, then take the fastest of three runs so unrelated test
        // parallelism does not make this timing assertion flaky.
        let mut best = std::time::Duration::MAX;
        let mut html = String::new();
        for _ in 0..3 {
            let start = std::time::Instant::now();
            html = diff_html(&old, &new);
            best = best.min(start.elapsed());
        }

        assert!(html.contains("<del"));
        assert!(
            best < std::time::Duration::from_millis(20),
            "3k-character diff took {best:?}"
        );
    }
}
