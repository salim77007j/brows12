//! Float layout regression tests (CSS 2.1 §9.5).
use brows12_css::values::FloatSide;
use brows12_css::{compute_styles, computed::CascadeCtx, StyleEngine, Stylesheet};
use brows12_html::parse_document;
use brows12_layout::{compute_layout, TextMeasurer, Viewport};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

fn layout_html(html: &str) -> (brows12_html::Document, brows12_layout::LayoutResult) {
    let doc = parse_document(html);
    let sheet =
        Stylesheet::parse("body { margin: 0; }", brows12_css::Origin::Author).unwrap();
    let engine = StyleEngine::with_author_sheets(&[sheet]);
    let ctx = CascadeCtx { viewport_width: 800.0, viewport_height: 600.0, ..Default::default() };
    let styles = compute_styles(&doc, &engine, &ctx);
    let measurer = TextMeasurer::new(Arc::new(Mutex::new(cosmic_text::FontSystem::new())));
    let result = compute_layout(&doc, &styles, Viewport { width: 800.0, height: 600.0 }, &measurer, &HashMap::new());
    (doc, result)
}

const FLOAT_DOC: &str = r#"<html><body>
  <div id="box" style="float: right; width: 200px; height: 100px; background:#eee">f</div>
  <p id="p1">aaaa aaaa aaaa aaaa aaaa aaaa aaaa aaaa aaaa aaaa aaaa aaaa aaaa aaaa aaaa aaaa aaaa aaaa aaaa aaaa aaaa aaaa aaaa aaaa aaaa aaaa aaaa aaaa aaaa aaaa aaaa aaaa aaaa aaaa aaaa aaaa aaaa aaaa aaaa aaaa aaaa aaaa aaaa aaaa aaaa aaaa aaaa aaaa aaaa aaaa aaaa aaaa aaaa aaaa aaaa aaaa aaaa aaaa aaaa aaaa aaaa aaaa aaaa aaaa aaaa aaaa aaaa aaaa aaaa aaaa aaaa aaaa aaaa aaaa aaaa aaaa aaaa aaaa aaaa aaaa aaaa aaaa aaaa aaaa aaaa aaaa aaaa aaaa aaaa aaaa aaaa aaaa aaaa aaaa aaaa aaaa aaaa aaaa aaaa aaaa aaaa aaaa aaaa aaaa aaaa aaaa aaaa aaaa aaaa aaaa aaaa aaaa aaaa</p>
  <div id="clearme" style="clear: both">cleared</div>
</body></html>"#;

#[test]
fn float_resolves_in_cascade() {
    let doc = parse_document("<div style=\"float: right\">x</div>");
    let engine = StyleEngine::with_author_sheets(&[]);
    let styles = compute_styles(&doc, &engine, &CascadeCtx::default());
    let div = doc.get_elements_by_tag_name("div")[0];
    assert!(matches!(styles.get(div).unwrap().float, FloatSide::Right));
}

#[test]
fn float_is_placed_at_container_edge() {
    let (doc, result) = layout_html(FLOAT_DOC);
    let box_node = doc.get_elements_by_tag_name("div")[0];
    let r = result.rect(box_node).unwrap();
    // body content width = 800; right float hugs the right edge
    assert!((r.x + r.width - 800.0).abs() < 2.0, "right float at right edge, got {:?}", r);
    assert_eq!(result.floats.len(), 1, "one float registered");
    assert!(!result.floats[0].left, "registered as a right float");
}

#[test]
fn text_wraps_around_right_float() {
    let (doc, result) = layout_html(FLOAT_DOC);
    let ps = doc.get_elements_by_tag_name("p");
    let p1 = ps[0];
    // Inline flows are keyed by the group's first member (the text node).
    let text_node = doc.node(p1).children[0];
    let flow = result
        .inline_flows
        .get(&text_node)
        .expect("paragraph has an inline flow");
    // With a 200px right float, lines must be shortened (left_inset 0,
    // right_inset ~200) for at least the first line.
    let first = &flow.lines[0];
    assert!(first.right_inset > 100.0, "line shortens around right float, got {}", first.right_inset);
    // Float height is 100px (~4-5 lines of 16px text); later lines, below
    // the float, must be full width again.
    let last = flow.lines.last().unwrap();
    assert!(last.right_inset < 1.0, "lines below the float are full width");
}

#[test]
fn clear_pushes_block_below_float() {
    let (doc, result) = layout_html(FLOAT_DOC);
    let divs = doc.get_elements_by_tag_name("div");
    let clearme = divs[1];
    let r = result.rect(clearme).unwrap();
    let float_bottom = result.floats[0].rect.y + result.floats[0].rect.height;
    assert!(r.y >= float_bottom - 1.0, "cleared block starts below the float: {:?} vs {}", r.y, float_bottom);
}
