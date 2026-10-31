//! Snapshot tests for the HTML produced by the diffing engine.
//!
//! These pin the exact markup for Vietnamese diacritics, escaped injection
//! attempts, line alignment, whitespace rendering, and the changes-only view.

use dp_core::diff::{DiffOptions, DiffRow, diff_html, diff_rows};

fn render_rows(rows: &[DiffRow]) -> String {
    let mut out = String::new();
    for row in rows {
        out.push_str(&format!(
            "[{}] old={:?} new={:?}\n  old: {}\n  new: {}\n",
            row.kind.as_str(),
            row.old_line_number,
            row.new_line_number,
            row.old_html,
            row.new_html
        ));
    }
    out
}

#[test]
fn snapshot_word_diff_with_vietnamese_diacritics() {
    insta::assert_snapshot!(
        "word_diff_vietnamese",
        diff_html(
            "Trăm năm trong cõi người ta,\nChữ tài chữ mệnh khéo là ghét nhau.",
            "Trăm năm trong cõi người ta,\nChữ tài chữ mệnh khéo là thương nhau."
        )
    );
}

#[test]
fn snapshot_line_rows_with_injected_html() {
    let rows = diff_rows(
        "Dòng một <script>alert('x')</script>\nDòng hai\nDòng ba",
        "Dòng một <script>alert('x')</script>\nDòng hai đã sửa\nDòng bốn",
        DiffOptions::default(),
    );
    insta::assert_snapshot!("line_rows_with_injected_html", render_rows(&rows));
}

#[test]
fn snapshot_changes_only_and_visible_whitespace() {
    let rows = diff_rows(
        "Giữ nguyên dòng này\nmột hai ba\nXoá dòng này",
        "Giữ nguyên dòng này\nmột  hai  ba",
        DiffOptions {
            show_whitespace: true,
            changes_only: true,
        },
    );
    insta::assert_snapshot!("changes_only_and_visible_whitespace", render_rows(&rows));
}
