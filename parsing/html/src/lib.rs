//! # brows12-html
//!
//! Arena-based DOM implementation and spec-compliant HTML5 parsing for the
//! Brows12 browser engine.
//!
//! The DOM is stored in a flat `Vec<Node>` arena: nodes reference each other
//! by [`NodeId`] instead of `Rc`/`Weak` cycles. This keeps traversal cache
//! friendly, makes mutation trivially safe, and means the whole document can
//! be dropped in O(1) — an important property for the engine's aggressive tab
//! suspension and memory-budgeting strategy.
//!
//! Parsing is delegated to `html5ever` (the Servo project's spec-compliant
//! HTML5 tokenizer + tree builder). We parse into `markup5ever_rcdom`'s
//! reference DOM and then transplant it into our arena, which keeps the
//! fragile HTML5 error-correction logic 100% upstream while giving us a DOM
//! shape designed for layout, style and JS binding.

pub mod dom;
pub mod parse;

pub use dom::{Document, Node, NodeData, NodeId};
pub use parse::{parse_document, parse_fragment};

/// The HTML namespace URL.
pub const HTML_NAMESPACE: &str = "http://www.w3.org/1999/xhtml";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_basic_document() {
        let doc = parse_document("<!DOCTYPE html><html><head><title>T</title></head><body><p id=\"a\">Hello</p></body></html>");
        let body = doc.body().expect("body element");
        assert_eq!(doc.local_name(body), "body");
        let p = doc.get_element_by_id("a").expect("p element");
        assert_eq!(doc.local_name(p), "p");
        assert_eq!(doc.text_content(p).trim(), "Hello");
        assert_eq!(doc.title(), Some("T".to_string()));
    }

    #[test]
    fn parser_error_correction() {
        // Mismatched tags are corrected per the HTML5 spec.
        let doc = parse_document("<html><body><b><i>x</b>y</i></body></html>");
        assert!(doc.get_elements_by_tag_name("i").len() >= 1);
        assert!(doc.get_elements_by_tag_name("b").len() >= 1);
        assert!(doc.text_content(doc.body().unwrap()).contains('y'));
    }

    #[test]
    fn fragment_parsing() {
        let doc = parse_document("<html><body></body></html>");
        let body = doc.body().unwrap();
        let nodes = parse_fragment(&mut doc.clone(), body, "<span>a</span><span>b</span>");
        assert_eq!(nodes.len(), 2);
    }
}
