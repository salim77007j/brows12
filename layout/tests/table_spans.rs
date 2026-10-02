//! Table colspan/rowspan regression tests (CSS 2.1 §17).
use brows12_css::{compute_styles, computed::CascadeCtx, StyleEngine, Stylesheet};
use brows12_html::parse_document;
use brows12_layout::{compute_layout, TextMeasurer, Viewport};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

const CSS: &str = "table { border-collapse: collapse } td { padding: 4px }";

const DOC: &str = r#"<html><body>
  <table>
    <tr><td id="a">a</td><td id="b">b</td><td id="c">c</td></tr>
    <tr><td id="d" colspan="2">d spans</td><td id="e">e</td></tr>
    <tr><td id="f">f</td><td id="g" rowspan="2">g spans down</td><td id="h">h</td></tr>
    <tr><td id="i">i</td><td id="j">j</td></tr>
  </table>
</body></html>"#;

fn layout() -> (brows12_html::Document, brows12_layout::LayoutResult) {
    let doc = parse_document(DOC);
    let sheet = Stylesheet::parse(CSS, brows12_css::Origin::Author).unwrap();
    let engine = StyleEngine::with_author_sheets(&[sheet]);
    let ctx = CascadeCtx { viewport_width: 800.0, viewport_height: 600.0, ..Default::default() };
    let styles = compute_styles(&doc, &engine, &ctx);
    let measurer = TextMeasurer::new(Arc::new(Mutex::new(cosmic_text::FontSystem::new())));
    let result = compute_layout(&doc, &styles, Viewport { width: 800.0, height: 600.0 }, &measurer, &HashMap::new());
    (doc, result)
}

fn id(doc: &brows12_html::Document, result: &brows12_layout::LayoutResult, name: &str) -> brows12_layout::Rect {
    let n = doc
        .get_elements_by_tag_name("td")
        .into_iter()
        .find(|&n| doc.attr(n, "id") == Some(name))
        .expect("cell exists");
    result.rect(n).expect("cell laid out")
}

#[test]
fn colspan_cell_spans_two_columns() {
    let (doc, result) = layout();
    let d = id(&doc, &result, "d");
    let e = id(&doc, &result, "e");
    // d starts in column 1 and stretches across columns 1-2: its right edge
    // reaches (or passes) e's left edge minus the gap.
    assert!(d.width > e.width, "spanning cell is wider: d={:?} e={:?}", d.width, e.width);
    assert!(e.x >= d.x + d.width - 2.0, "e starts after the spanning cell");
}

#[test]
fn rowspan_cell_covers_next_row() {
    let (doc, result) = layout();
    let g = id(&doc, &result, "g");
    let i = id(&doc, &result, "i");
    let j = id(&doc, &result, "j");
    // g covers rows 3-4: taller than a single-row cell; i/j sit below it.
    assert!(g.height > i.height + 10.0, "rowspan cell is taller: g={:?}", g.height);
    assert!(i.y >= g.y + g.height - 2.0 || i.x < g.x, "row 4 cells flow beside/below g");
    // j occupies the last column of row 4 — same column as h above it.
    let h = id(&doc, &result, "h");
    assert!((j.x - h.x).abs() < 2.0, "row 4 last cell aligns with row 3 column 3");
}
