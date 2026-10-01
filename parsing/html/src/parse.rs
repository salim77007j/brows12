//! HTML5 parsing into the Brows12 arena DOM.

use crate::dom::{Document, NodeData, NodeId};
use html5ever::driver::{
    parse_document as he_parse_document, parse_fragment as he_parse_fragment, ParseOpts,
};
use html5ever::tendril::TendrilSink;
use markup5ever_rcdom::{Handle, NodeData as RcNodeData, RcDom};

/// Parse a full HTML document string into an arena [`Document`].
pub fn parse_document(html: &str) -> Document {
    let dom: RcDom = he_parse_document(RcDom::default(), ParseOpts::default()).one(html);
    let mut doc = Document::new();
    let root = doc.root();
    transplant(&mut doc, dom.document, root);
    doc
}

/// Parse an HTML fragment in the context of `context_element` and append the
/// resulting top-level nodes to it. Returns the newly created node ids.
pub fn parse_fragment(doc: &mut Document, context_element: NodeId, html: &str) -> Vec<NodeId> {
    let context_name = html5ever::QualName::new(
        None,
        html5ever::Namespace::from(crate::HTML_NAMESPACE),
        html5ever::LocalName::from(doc.local_name(context_element)),
    );
    let dom: RcDom = he_parse_fragment(
        RcDom::default(),
        ParseOpts::default(),
        context_name,
        Vec::new(),
        true,
    )
    .one(html);

    // html5ever wraps fragment output in a synthetic `<html>` root; unwrap it
    // so callers see exactly the nodes they parsed.
    let top = top_level_fragment_nodes(&dom);
    let before = doc.node(context_element).children.len();
    for child in top {
        let id = convert(doc, child.clone());
        doc.append_child(context_element, id);
        transplant_children(doc, child, id);
    }
    doc.node(context_element).children[before..].to_vec()
}

fn top_level_fragment_nodes(dom: &RcDom) -> Vec<Handle> {
    let children = dom.document.children.borrow().clone();
    if children.len() == 1 {
        if let RcNodeData::Element { name, .. } = &children[0].data {
            if &*name.local == "html" {
                return children[0].children.borrow().clone();
            }
        }
    }
    children
}

fn transplant(doc: &mut Document, handle: Handle, into: NodeId) {
    match &handle.data {
        RcNodeData::Document => {
            for child in handle.children.borrow().iter() {
                transplant(doc, child.clone(), into);
            }
        }
        _ => {
            let id = convert(doc, handle.clone());
            doc.append_child(into, id);
            transplant_children(doc, handle, id);
        }
    }
}

fn transplant_children(doc: &mut Document, handle: Handle, into: NodeId) {
    for child in handle.children.borrow().iter() {
        transplant(doc, child.clone(), into);
    }
}

fn convert(doc: &mut Document, handle: Handle) -> NodeId {
    let data = match &handle.data {
        RcNodeData::Document => NodeData::Document,
        RcNodeData::Doctype { name, .. } => NodeData::Doctype {
            name: name.to_string(),
        },
        RcNodeData::Text { contents } => NodeData::Text(contents.borrow().to_string()),
        RcNodeData::Comment { contents } => NodeData::Comment(contents.to_string()),
        RcNodeData::ProcessingInstruction { target, contents } => NodeData::Comment(format!(
            "<?{} {}?>",
            target,
            contents
        )),
        RcNodeData::Element { name, attrs, .. } => {
            let tag = if &*name.ns == crate::HTML_NAMESPACE {
                name.local.to_string()
            } else {
                format!("{} {}", name.ns, name.local).trim().to_string()
            };
            let mut out = Vec::with_capacity(attrs.borrow().len());
            for attr in attrs.borrow().iter() {
                out.push((attr.name.local.to_string(), attr.value.to_string()));
            }
            NodeData::Element { name: tag, attrs: out }
        }
    };
    doc.create_node(data)
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn document_with_doctype_and_comments() {
        let doc = parse_document("<!--hi--><!DOCTYPE html><html><body><p>x</p></body></html>");
        assert!(doc.body().is_some());
        assert!(doc.get_elements_by_tag_name("p").len() == 1);
    }

    #[test]
    fn fragment_in_div_context() {
        let mut doc = parse_document("<html><body><div></div></body></html>");
        let div = doc.get_elements_by_tag_name("div")[0];
        let created = parse_fragment(&mut doc, div, "<b>bold</b> tail");
        assert_eq!(created.len(), 2);
        assert_eq!(doc.local_name(created[0]), "b");
        assert_eq!(doc.text_content(div).trim(), "bold tail");
    }

    #[test]
    fn head_and_title_extraction() {
        let doc = parse_document("<html><head><meta charset=\"utf-8\"><title>Page</title></head><body></body></html>");
        assert_eq!(doc.title().as_deref(), Some("Page"));
        assert!(doc.head().is_some());
    }
}
