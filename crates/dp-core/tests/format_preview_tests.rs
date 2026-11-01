//! Snapshot and invariant tests for the Phase 9 format preview.

use dp_core::format_preview::{parse_preview, render_preview};

const RICH_PAGE: &str = "\
Truyện Kiều — Nguyễn Du

_Trăm năm_ trong cõi người ta,
*Chữ tài* chữ mệnh khéo là ghét nhau.[Footnote 1: Câu mở đầu của truyện.]

# Đoạn trích

/>  Người ta là hoa đất
/>    Đất lành chim đậu

----

<sc>Hết</sc>.";

#[test]
fn snapshot_rich_page() {
    insta::assert_snapshot!("format_preview_rich", render_preview(RICH_PAGE));
}

#[test]
fn snapshot_unclosed_markers() {
    let source = "_nghiêng mà không đóng\n\n*đậm mà không đóng\n\n<sc>nhỏ mà không đóng";
    insta::assert_snapshot!("format_preview_unclosed", render_preview(source));
}

#[test]
fn snapshot_escaping() {
    let source = "<script>alert('x')</script>\n\n<img src=x onerror=alert(1)>";
    insta::assert_snapshot!("format_preview_escaping", render_preview(source));
}

#[test]
fn parsed_blocks_cover_the_page() {
    let blocks = parse_preview(RICH_PAGE);
    assert!(
        blocks.len() >= 6,
        "expected all block kinds, got {blocks:#?}"
    );
}

#[test]
fn every_rendered_fragment_is_self_contained() {
    // Rendering a fragment twice must not accumulate footnote state.
    let once = render_preview(RICH_PAGE);
    let twice = render_preview(RICH_PAGE);
    assert_eq!(once, twice);
    assert!(once.contains("<ol class=\"footnotes\">"));
}
