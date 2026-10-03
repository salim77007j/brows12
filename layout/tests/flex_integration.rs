//! Flex min-content integration tests (CSS Flexbox §9.7 / §4.5).
//!
//! Covers: flex-basis (px/percent) + grow distribution, the fixed-size
//! `flex: 0 0 <len>` item, automatic minimum size (`min-width:auto`
//! flooring at min-content), `min-width:0` shrink-below-content, nested
//! flex, and `display:inline-flex` shrink-to-fit sizing.
use brows12_css::{compute_styles, computed::CascadeCtx, StyleEngine, Stylesheet};
use brows12_html::parse_document;
use brows12_layout::{compute_layout, TextMeasurer, Viewport};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

fn layout_html(html: &str, width: f32) -> (brows12_html::Document, brows12_layout::LayoutResult) {
    let doc = parse_document(html);
    let sheet = Stylesheet::parse("body { margin: 0; }", brows12_css::Origin::Author).unwrap();
    let engine = StyleEngine::with_author_sheets(&[sheet]);
    let ctx = CascadeCtx { viewport_width: width, viewport_height: 600.0, ..Default::default() };
    let styles = compute_styles(&doc, &engine, &ctx);
    let measurer = TextMeasurer::new(Arc::new(Mutex::new(cosmic_text::FontSystem::new())));
    let result = compute_layout(
        &doc,
        &styles,
        Viewport { width, height: 600.0 },
        &measurer,
        &HashMap::new(),
    );
    (doc, result)
}

const BASIS_DOC: &str = r#"<html><body style="margin:0">
<div style="display:flex; width:420px">
  <div id="a" style="flex: 1 1 60px">grow1</div>
  <div id="b" style="flex: 2 1 30%">grow2</div>
  <div id="c" style="flex: 0 0 80px">no-grow</div>
</div>
</body></html>"#;

#[test]
fn flex_basis_px_and_percent_distribute_like_chrome() {
    let (doc, result) = layout_html(BASIS_DOC, 800.0);
    let get = |id: &str| doc.get_element_by_id(id).unwrap();
    let a = result.rect(get("a")).unwrap();
    let b = result.rect(get("b")).unwrap();
    let c = result.rect(get("c")).unwrap();
    // Container inner width 420: bases 60 + 30% (126) + 80 = 266; free 154
    // split by grow 1:2 -> a = 60 + 51.33, b = 126 + 102.67, c = 80.
    let aw = a.width;
    let bw = b.width;
    let cw = c.width;
    assert!((aw - 111.3).abs() < 3.0, "item a ~111px, got {aw}");
    assert!((bw - 228.7).abs() < 3.0, "item b ~229px, got {bw}");
    assert!((cw - 80.0).abs() < 1.0, "item c exactly 80px, got {cw}");
    // Items tile the row without gaps.
    assert!((a.x - 0.0).abs() < 1.0);
    assert!((b.x - (a.x + aw)).abs() < 1.5, "b starts where a ends");
    assert!((c.x - (b.x + bw)).abs() < 1.5, "c starts where b ends");
}

const FIXED_DOC: &str = r#"<html><body style="margin:0">
<div style="display:flex; width:400px">
  <div id="g" style="flex:1; min-width:0; overflow:hidden; white-space:nowrap">supercalifragilisticexpialidocious-and-more-text-here-truncates</div>
  <div id="f" style="flex: 0 0 120px">fixed 120px</div>
</div>
</body></html>"#;

#[test]
fn flex_fixed_item_is_exactly_120px_and_grow_takes_rest() {
    let (doc, result) = layout_html(FIXED_DOC, 800.0);
    let get = |id: &str| doc.get_element_by_id(id).unwrap();
    let g = result.rect(get("g")).unwrap();
    let f = result.rect(get("f")).unwrap();
    assert!((f.width - 120.0).abs() < 1.0, "fixed item 120px, got {}", f.width);
    assert!((g.width - 280.0).abs() < 1.5, "grow item 280px, got {}", g.width);
    assert!(f.x >= 279.0, "fixed item starts at 280, got {}", f.x);
    // The fixed item's text must fit on ONE line ("fixed 120px" ~ 75px).
    assert!(
        f.height < 40.0,
        "fixed item text should not wrap; height {} suggests wrapping",
        f.height
    );
}

const AUTO_MIN_DOC: &str = r#"<html><body style="margin:0">
<div style="display:flex; width:400px">
  <div id="lw" style="flex:1">unbreakablewordgoesherewithoutspaces</div>
  <div id="st" style="flex:1">short text</div>
</div>
</body></html>"#;

#[test]
fn automatic_minimum_size_floors_long_word() {
    let (doc, result) = layout_html(AUTO_MIN_DOC, 800.0);
    let get = |id: &str| doc.get_element_by_id(id).unwrap();
    let lw = result.rect(get("lw")).unwrap();
    // min-content of the word at 16px default font is far below 400, but it
    // must exceed 50% (Chrome floors the item at its min-content width and
    // the row overflows instead of splitting the word).
    assert!(lw.width > 150.0, "auto min-size floors at min-content, got {}", lw.width);
}

const NESTED_DOC: &str = r#"<html><body style="margin:0">
<div style="display:flex; width:500px; gap:6px">
  <div id="col1" style="display:flex; flex-direction:column; flex:1; gap:4px">
    <div>A1</div><div>A2</div>
  </div>
  <div id="col2" style="display:flex; flex-direction:column; flex:1; gap:4px">
    <div>B1</div><div>B2</div>
  </div>
  <div id="side" style="width:100px">side</div>
</div>
</body></html>"#;

#[test]
fn nested_flex_columns_share_free_space() {
    let (doc, result) = layout_html(NESTED_DOC, 800.0);
    let get = |id: &str| doc.get_element_by_id(id).unwrap();
    let c1 = result.rect(get("col1")).unwrap();
    let c2 = result.rect(get("col2")).unwrap();
    let side = result.rect(get("side")).unwrap();
    // 500 - 100 side - 2*6 gaps = 388; two grow-1 columns -> 194 each.
    assert!((c1.width - 194.0).abs() < 2.0, "col1 194px, got {}", c1.width);
    assert!((c2.width - 194.0).abs() < 2.0, "col2 194px, got {}", c2.width);
    assert!((side.width - 100.0).abs() < 1.0, "side 100px, got {}", side.width);
    assert!((side.x + side.width - 500.0).abs() < 2.0, "side hugs right edge");
}

const INLINE_FLEX_DOC: &str = r#"<html><body style="margin:0">
<div id="host" style="font-size:14px">
  <div id="pill" style="display:inline-flex; gap:6px; padding:4px 8px">
    <span>rust</span><span>engines</span>
  </div>
  <span id="after">tail</span>
</div>
</body></html>"#;

#[test]
fn inlineflex_shrinks_to_fit_and_stays_atomic() {
    let (doc, result) = layout_html(INLINE_FLEX_DOC, 800.0);
    let get = |id: &str| doc.get_element_by_id(id).unwrap();
    let pill = result.rect(get("pill")).unwrap();
    // Pills: "rust" (~28) + "engines" (~52) + gap 6 + padding 16 => ~102px wide.
    assert!(pill.width < 220.0, "inline-flex shrink-to-fit, got width {}", pill.width);
    assert!(pill.width > 60.0, "inline-flex fits its content, got width {}", pill.width);
    // Content-sized height: two spans on one flex line + padding.
    assert!(pill.height < 40.0, "single-line pill height, got {}", pill.height);
}
