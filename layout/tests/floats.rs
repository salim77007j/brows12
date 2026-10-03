//! Float layout regression tests (CSS 2.1 §9.5).
use brows12_css::values::FloatSide;
use brows12_css::{compute_styles, computed::CascadeCtx, StyleEngine, Stylesheet};
use brows12_html::parse_document;
use brows12_layout::{compute_layout, TextMeasurer, Viewport};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

fn layout_html(html: &str) -> (brows12_html::Document, brows12_layout::LayoutResult) {
    let doc = parse_document(html);
    let sheet = Stylesheet::parse("body { margin: 0; }", brows12_css::Origin::Author).unwrap();
    let engine = StyleEngine::with_author_sheets(&[sheet]);
    let ctx = CascadeCtx { viewport_width: 800.0, viewport_height: 600.0, ..Default::default() };
    let styles = compute_styles(&doc, &engine, &ctx);
    let measurer = TextMeasurer::new(Arc::new(Mutex::new(cosmic_text::FontSystem::new())));
    let result = compute_layout(
        &doc,
        &styles,
        Viewport { width: 800.0, height: 600.0 },
        &measurer,
        &HashMap::new(),
    );
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
    let flow = result.inline_flows.get(&text_node).expect("paragraph has an inline flow");
    // With a 200px right float, lines must be shortened (left_inset 0,
    // right_inset ~200) for at least the first line.
    let first = &flow.lines[0];
    assert!(
        first.right_inset > 100.0,
        "line shortens around right float, got {}",
        first.right_inset
    );
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
    assert!(
        r.y >= float_bottom - 1.0,
        "cleared block starts below the float: {:?} vs {}",
        r.y,
        float_bottom
    );
}

const GRID_DOC: &str = r#"<html><body>
  <div class="g">
    <div id="a" class="c1">a</div>
    <div id="b">b</div>
    <div id="c">c</div>
    <div id="d">d</div>
    <div id="e">e</div>
  </div>
</body></html>"#;

const GRID_CSS: &str = ".g { display: grid; grid-template-columns: 100px 1fr 1fr; gap: 10px; } .c1 { grid-row: 1 / span 2; }";

fn layout_doc(css: &str) -> (brows12_html::Document, brows12_layout::LayoutResult) {
    let doc = parse_document(GRID_DOC);
    let sheet = Stylesheet::parse(css, brows12_css::Origin::Author).unwrap();
    let engine = StyleEngine::with_author_sheets(&[sheet]);
    let ctx = CascadeCtx { viewport_width: 800.0, viewport_height: 600.0, ..Default::default() };
    let styles = compute_styles(&doc, &engine, &ctx);
    let measurer = TextMeasurer::new(Arc::new(Mutex::new(cosmic_text::FontSystem::new())));
    let result = compute_layout(
        &doc,
        &styles,
        Viewport { width: 800.0, height: 600.0 },
        &measurer,
        &HashMap::new(),
    );
    (doc, result)
}

#[test]
fn grid_tracks_and_placement() {
    let (doc, result) = layout_doc(GRID_CSS);
    let ids = |id: &str| {
        doc.get_elements_by_tag_name("div")
            .into_iter()
            .find(|&n| doc.attr(n, "id") == Some(id))
            .unwrap()
    };
    let a = result.rect(ids("a")).unwrap();
    let b = result.rect(ids("b")).unwrap();
    let c = result.rect(ids("c")).unwrap();
    let d = result.rect(ids("d")).unwrap();
    // Column 1 = 100px; gap 10 → column 2 starts at 110.
    assert!((b.x - 110.0).abs() < 2.0, "second column after 100px + gap, got {:?}", b.x);
    // `a` spans rows 1-2: taller than single-row items.
    assert!(a.height > d.height + 20.0, "row span grows the item: a={:?} d={:?}", a, d);
    // `d` sits on row 2 column 1 (below the spanning `a`).
    assert!(d.y > b.y, "d is on a later row");
    // fr columns split the remaining width equally; gaps are honored.
    let e = result.rect(ids("e")).unwrap();
    assert!(
        (c.width - b.width).abs() < 2.0,
        "fr columns are equal: b={:?} c={:?}",
        b.width,
        c.width
    );
    assert!(
        (c.x - b.x - b.width - 10.0).abs() < 2.0,
        "gap between fr columns, got {:?}",
        c.x - b.x - b.width
    );
    assert!((e.x - d.x - d.width - 10.0).abs() < 2.0, "gap on row 2");
}
