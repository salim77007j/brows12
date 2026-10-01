//! Arena-backed DOM tree.

use std::fmt::Write as _;

/// Stable identifier of a node inside a [`Document`] arena.
///
/// Ids are never reused within a document, which makes them safe to hand out
/// to the JavaScript bindings, the style engine and the layout engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId(pub u32);

/// Data carried by a single DOM node.
#[derive(Debug, Clone, PartialEq)]
pub enum NodeData {
    /// The document root.
    Document,
    /// A `<!DOCTYPE html>` declaration.
    Doctype { name: String },
    /// An element with attributes. Attribute names are lowercased.
    Element { name: String, attrs: Vec<(String, String)> },
    /// A text node.
    Text(String),
    /// A comment node.
    Comment(String),
}

/// One node of the DOM tree.
#[derive(Debug, Clone)]
pub struct Node {
    /// Location of this node in the arena.
    pub id: NodeId,
    /// Parent node, if any.
    pub parent: Option<NodeId>,
    /// Child nodes in document order.
    pub children: Vec<NodeId>,
    /// Payload of this node.
    pub data: NodeData,
}

/// An arena-owned HTML document.
#[derive(Debug, Clone)]
pub struct Document {
    nodes: Vec<Node>,
}

impl Default for Document {
    fn default() -> Self {
        Self::new()
    }
}

impl Document {
    /// Create an empty document containing only the document root node.
    pub fn new() -> Self {
        Self {
            nodes: vec![Node {
                id: NodeId(0),
                parent: None,
                children: Vec::new(),
                data: NodeData::Document,
            }],
        }
    }

    // -- arena access ------------------------------------------------------

    /// Borrow a node by id.
    #[inline]
    pub fn node(&self, id: NodeId) -> &Node {
        &self.nodes[id.0 as usize]
    }

    /// Mutably borrow a node by id.
    #[inline]
    pub fn node_mut(&mut self, id: NodeId) -> &mut Node {
        &mut self.nodes[id.0 as usize]
    }

    /// Total number of allocated nodes (used by memory budgeting).
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// The document root node.
    pub fn root(&self) -> NodeId {
        NodeId(0)
    }

    fn alloc(&mut self, data: NodeData) -> NodeId {
        let id = NodeId(self.nodes.len() as u32);
        self.nodes.push(Node {
            id,
            parent: None,
            children: Vec::new(),
            data,
        });
        id
    }

    // -- construction ------------------------------------------------------

    /// Create a detached element node.
    pub fn create_element(&mut self, tag: &str) -> NodeId {
        self.alloc(NodeData::Element {
            name: tag.to_ascii_lowercase(),
            attrs: Vec::new(),
        })
    }

    /// Create a detached text node.
    pub fn create_text_node(&mut self, text: &str) -> NodeId {
        self.alloc(NodeData::Text(text.to_string()))
    }

    /// Create a detached comment node.
    pub fn create_comment(&mut self, text: &str) -> NodeId {
        self.alloc(NodeData::Comment(text.to_string()))
    }

    /// Create a detached node from arbitrary data (used by the parser).
    pub fn create_node(&mut self, data: NodeData) -> NodeId {
        self.alloc(data)
    }

    /// Append `child` as the last child of `parent`.
    pub fn append_child(&mut self, parent: NodeId, child: NodeId) {
        if let Some(old) = self.node(child).parent {
            self.detach(old, child);
        }
        self.node_mut(child).parent = Some(parent);
        self.node_mut(parent).children.push(child);
    }

    /// Insert `child` before `reference` inside the same parent. If the
    /// reference is missing the child is appended instead.
    pub fn insert_before(&mut self, parent: NodeId, child: NodeId, reference: NodeId) {
        if child == reference {
            return;
        }
        if let Some(old) = self.node(child).parent {
            self.detach(old, child);
        }
        self.node_mut(child).parent = Some(parent);
        let idx = self.node(parent)
            .children
            .iter()
            .position(|&c| c == reference)
            .unwrap_or_else(|| self.node(parent).children.len());
        self.node_mut(parent).children.insert(idx, child);
    }

    /// Remove `child` from `parent`. Returns true when the child was present.
    pub fn remove_child(&mut self, parent: NodeId, child: NodeId) -> bool {
        if self.node(child).parent != Some(parent) {
            return false;
        }
        self.detach(parent, child);
        true
    }

    fn detach(&mut self, parent: NodeId, child: NodeId) {
        if let Some(idx) = self.node(parent).children.iter().position(|&c| c == child) {
            self.node_mut(parent).children.remove(idx);
        }
        self.node_mut(child).parent = None;
    }

    // -- traversal ---------------------------------------------------------

    /// Element children of `id` in document order.
    pub fn child_elements(&self, id: NodeId) -> Vec<NodeId> {
        self.node(id)
            .children
            .iter()
            .copied()
            .filter(|&c| self.is_element(c))
            .collect()
    }

    /// All children of `id`.
    pub fn children(&self, id: NodeId) -> &[NodeId] {
        &self.node(id).children
    }

    /// Parent of `id`, if attached.
    pub fn parent(&self, id: NodeId) -> Option<NodeId> {
        self.node(id).parent
    }

    /// First element child of `id`.
    pub fn first_element_child(&self, id: NodeId) -> Option<NodeId> {
        self.child_elements(id).first().copied()
    }

    /// Next element sibling of `id`.
    pub fn next_sibling_element(&self, id: NodeId) -> Option<NodeId> {
        let parent = self.node(id).parent?;
        let sibs = &self.node(parent).children;
        let idx = sibs.iter().position(|&c| c == id)?;
        sibs[idx + 1..]
            .iter()
            .copied()
            .find(|&c| self.is_element(c))
    }

    /// Previous element sibling of `id`.
    pub fn previous_sibling_element(&self, id: NodeId) -> Option<NodeId> {
        let parent = self.node(id).parent?;
        let sibs = &self.node(parent).children;
        let idx = sibs.iter().position(|&c| c == id)?;
        sibs[..idx].iter().copied().rev().find(|&c| self.is_element(c))
    }

    /// Is this node an element?
    #[inline]
    pub fn is_element(&self, id: NodeId) -> bool {
        matches!(self.node(id).data, NodeData::Element { .. })
    }

    /// Is this node a text node?
    #[inline]
    pub fn is_text(&self, id: NodeId) -> bool {
        matches!(self.node(id).data, NodeData::Text(_))
    }

    /// Local (lowercased) tag name of an element. Empty for non-elements.
    pub fn local_name(&self, id: NodeId) -> &str {
        match &self.node(id).data {
            NodeData::Element { name, .. } => name,
            _ => "",
        }
    }

    /// Index of the element among its element siblings (for `:nth-child`).
    /// 1-based; returns 0 for detached nodes.
    pub fn element_index(&self, id: NodeId) -> usize {
        let Some(parent) = self.node(id).parent else {
            return 0;
        };
        let mut idx = 0;
        for &c in &self.node(parent).children {
            if c == id {
                return idx + 1;
            }
            if self.is_element(c) {
                idx += 1;
            }
        }
        0
    }

    /// Number of element children of the element's parent.
    pub fn sibling_element_count(&self, id: NodeId) -> usize {
        match self.node(id).parent {
            Some(parent) => self.child_elements(parent).len(),
            None => 0,
        }
    }

    // -- document level helpers --------------------------------------------

    /// The root `<html>` element.
    pub fn document_element(&self) -> Option<NodeId> {
        self.first_element_child(self.root())
    }

    /// The `<body>` element, if present.
    pub fn body(&self) -> Option<NodeId> {
        let html = self.document_element()?;
        self.child_elements(html)
            .into_iter()
            .find(|&c| self.local_name(c) == "body")
    }

    /// The `<head>` element, if present.
    pub fn head(&self) -> Option<NodeId> {
        let html = self.document_element()?;
        self.child_elements(html)
            .into_iter()
            .find(|&c| self.local_name(c) == "head")
    }

    /// Text of the `<title>` element.
    pub fn title(&self) -> Option<String> {
        let head = self.head()?;
        let title = self.child_elements(head)
            .into_iter()
            .find(|&c| self.local_name(c) == "title")?;
        Some(self.text_content(title))
    }

    // -- attributes ---------------------------------------------------------

    /// Value of attribute `name` on element `id` (case-insensitive).
    pub fn attr(&self, id: NodeId, name: &str) -> Option<&str> {
        match &self.node(id).data {
            NodeData::Element { attrs, .. } => attrs
                .iter()
                .find(|(n, _)| n.eq_ignore_ascii_case(name))
                .map(|(_, v)| v.as_str()),
            _ => None,
        }
    }

    /// All attributes of element `id`.
    pub fn attrs(&self, id: NodeId) -> &[(String, String)] {
        match &self.node(id).data {
            NodeData::Element { attrs, .. } => attrs,
            _ => &[],
        }
    }

    /// Set attribute `name` on element `id`.
    pub fn set_attr(&mut self, id: NodeId, name: &str, value: &str) {
        if let NodeData::Element { attrs, .. } = &mut self.node_mut(id).data {
            if let Some(slot) = attrs.iter_mut().find(|(n, _)| n.eq_ignore_ascii_case(name)) {
                slot.1 = value.to_string();
            } else {
                attrs.push((name.to_ascii_lowercase(), value.to_string()));
            }
        }
    }

    /// Remove attribute `name` from element `id`.
    pub fn remove_attr(&mut self, id: NodeId, name: &str) -> bool {
        if let NodeData::Element { attrs, .. } = &mut self.node_mut(id).data {
            let before = attrs.len();
            attrs.retain(|(n, _)| !n.eq_ignore_ascii_case(name));
            return attrs.len() != before;
        }
        false
    }

    /// Value of the `id` attribute, if any.
    pub fn element_id_attr(&self, id: NodeId) -> Option<&str> {
        self.attr(id, "id")
    }

    /// Class list of element `id`.
    pub fn classes(&self, id: NodeId) -> impl Iterator<Item = &str> {
        self.attr(id, "class")
            .unwrap_or("")
            .split_ascii_whitespace()
    }

    // -- text and lookup -----------------------------------------------------

    /// Concatenated text of `id` and all its descendant text nodes.
    pub fn text_content(&self, id: NodeId) -> String {
        let mut out = String::new();
        self.collect_text(id, &mut out);
        out
    }

    fn collect_text(&self, id: NodeId, out: &mut String) {
        match &self.node(id).data {
            NodeData::Text(t) => out.push_str(t),
            _ => {
                for &c in &self.node(id).children {
                    self.collect_text(c, out);
                }
            }
        }
    }

    /// Replace all children of `id` with a single text node.
    pub fn set_text_content(&mut self, id: NodeId, text: &str) {
        let children = self.node(id).children.clone();
        for c in children {
            self.remove_child(id, c);
        }
        let t = self.create_text_node(text);
        self.append_child(id, t);
    }

    /// First element with attribute `id="..."` equal to `value`.
    pub fn get_element_by_id(&self, value: &str) -> Option<NodeId> {
        self.walk(self.root(), &mut |n| {
            self.is_element(n) && self.attr(n, "id") == Some(value)
        })
    }

    /// All elements with the given lowercased tag name. `"*"` matches any.
    pub fn get_elements_by_tag_name(&self, tag: &str) -> Vec<NodeId> {
        let mut out = Vec::new();
        self.walk_collect(self.root(), tag, &mut out);
        out
    }

    fn walk_collect(&self, id: NodeId, tag: &str, out: &mut Vec<NodeId>) {
        if self.is_element(id) && (tag == "*" || self.local_name(id) == tag) {
            out.push(id);
        }
        for &c in &self.node(id).children {
            self.walk_collect(c, tag, out);
        }
    }

    fn walk(&self, id: NodeId, pred: &mut dyn FnMut(NodeId) -> bool) -> Option<NodeId> {
        if pred(id) {
            return Some(id);
        }
        for &c in &self.node(id).children {
            if let Some(found) = self.walk(c, pred) {
                return Some(found);
            }
        }
        None
    }

    /// Depth-first pre-order visit of all nodes.
    pub fn visit_all(&self, mut f: impl FnMut(NodeId)) {
        fn rec(doc: &Document, id: NodeId, f: &mut impl FnMut(NodeId)) {
            f(id);
            let children = doc.node(id).children.clone();
            for c in children {
                rec(doc, c, f);
            }
        }
        rec(self, self.root(), &mut f);
    }

    // -- serialization -------------------------------------------------------

    /// Serialize the children of `id` as HTML (the `innerHTML` property).
    pub fn inner_html(&self, id: NodeId) -> String {
        let mut out = String::new();
        for &c in &self.node(id).children {
            self.serialize_node(c, &mut out);
        }
        out
    }

    /// Serialize `id` including itself (the `outerHTML` property).
    pub fn outer_html(&self, id: NodeId) -> String {
        let mut out = String::new();
        self.serialize_node(id, &mut out);
        out
    }

    fn serialize_node(&self, id: NodeId, out: &mut String) {
        match &self.node(id).data {
            NodeData::Document | NodeData::Doctype { .. } => {
                for &c in &self.node(id).children {
                    self.serialize_node(c, out);
                }
            }
            NodeData::Text(t) => escape_text(t, out),
            NodeData::Comment(t) => {
                let _ = write!(out, "<!--{t}-->");
            }
            NodeData::Element { name, attrs } => {
                let _ = write!(out, "<{name}");
                for (n, v) in attrs {
                    let _ = write!(out, " {n}=\"{}\"", escape_attr(v));
                }
                // Void elements have no closing tag per the HTML spec.
                if VOID_ELEMENTS.contains(&name.as_str()) {
                    out.push('>');
                    return;
                }
                out.push('>');
                for &c in &self.node(id).children {
                    self.serialize_node(c, out);
                }
                let _ = write!(out, "</{name}>");
            }
        }
    }
}

const VOID_ELEMENTS: &[&str] = &[
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param",
    "source", "track", "wbr",
];

fn escape_text(s: &str, out: &mut String) {
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            _ => out.push(ch),
        }
    }
}

fn escape_attr(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tree_mutation() {
        let mut doc = Document::new();
        let root = doc.create_element("div");
        let a = doc.create_element("span");
        let b = doc.create_text_node("hi");
        doc.append_child(root, a);
        doc.append_child(root, b);
        assert_eq!(doc.node(root).children, vec![a, b]);
        doc.insert_before(root, b, a);
        assert_eq!(doc.node(root).children, vec![b, a]);
        assert!(doc.remove_child(root, b));
        assert_eq!(doc.node(root).children, vec![a]);
    }

    #[test]
    fn nth_child_indexing() {
        let mut doc = Document::new();
        let parent = doc.create_element("ul");
        let items: Vec<_> = (0..3)
            .map(|i| {
                let li = doc.create_element("li");
                doc.append_child(parent, li);
                li
            })
            .collect();
        let comment = doc.create_comment("c");
        doc.append_child(parent, comment);
        assert_eq!(doc.element_index(items[1]), 2);
        assert_eq!(doc.sibling_element_count(items[1]), 3);
        assert_eq!(doc.next_sibling_element(items[2]), None);
        assert_eq!(doc.previous_sibling_element(items[1]), Some(items[0]));
    }

    #[test]
    fn serialization_roundtrip() {
        let doc = crate::parse::parse_document("<html><body><p class=\"x\">a&amp;b</p></body></html>");
        let p = doc.get_elements_by_tag_name("p")[0];
        assert_eq!(doc.outer_html(p), "<p class=\"x\">a&amp;b</p>");
    }
}
