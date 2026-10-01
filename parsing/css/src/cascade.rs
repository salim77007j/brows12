//! Cascade: match rules, order them, apply declarations, inherit.

use crate::apply::apply_property;
use crate::computed::{CascadeCtx, ComputedStyle};
use crate::matcher;
use crate::stylesheet::{Origin, StyleEngine, Stylesheet};
use brows12_html::{Document, NodeData, NodeId};
use std::collections::HashMap;

/// Per-document style map: node id -> computed style.
#[derive(Debug, Clone, Default)]
pub struct StyleMap {
    pub styles: HashMap<NodeId, ComputedStyle>,
}

impl StyleMap {
    pub fn get(&self, node: NodeId) -> Option<&ComputedStyle> {
        self.styles.get(&node)
    }
}

/// Compute styles for every element (and text node) of `doc` using the
/// combined author + UA rule set of `engine`.
pub fn compute_styles(doc: &Document, engine: &StyleEngine, ctx: &CascadeCtx) -> StyleMap {
    let mut map = StyleMap::default();
    let root = doc.root();

    fn rec(
        doc: &Document,
        engine: &StyleEngine,
        ctx: &CascadeCtx,
        node: NodeId,
        parent_style: &ComputedStyle,
        map: &mut StyleMap,
    ) {
        let data = doc.node(node).data.clone();
        match &data {
            NodeData::Element { .. } => {
                let mut style = ComputedStyle::inherit_from(parent_style);
                style.font_size = parent_style.font_size;

                // Cascade matching rules.
                let mut matched: Vec<&crate::stylesheet::StyleRule> = engine
                    .rules()
                    .iter()
                    .filter(|rule| {
                        rule.selectors.0.iter().any(|sel| matcher::matches_selector(doc, node, sel))
                    })
                    .collect();
                // Cascade order: origin (UA < author), then specificity, then
                // source order. Stable sort keeps document order deterministic.
                matched.sort_by(|a, b| {
                    let origin_a = (a.origin == Origin::Author) as u8;
                    let origin_b = (b.origin == Origin::Author) as u8;
                    origin_a
                        .cmp(&origin_b)
                        .then(a.specificity.cmp(&b.specificity))
                        .then(a.order.cmp(&b.order))
                });

                for rule in &matched {
                    for prop in &rule.declarations {
                        apply_property(&mut style, prop, Some(parent_style), ctx);
                    }
                }
                for rule in &matched {
                    for prop in &rule.important {
                        apply_property(&mut style, prop, Some(parent_style), ctx);
                    }
                }

                // Inline style="" declarations win over all author rules.
                if let Some(inline) = doc.attr(node, "style") {
                    apply_inline(&mut style, inline, Some(parent_style), ctx);
                }

                map.styles.insert(node, style.clone());

                let children = doc.node(node).children.clone();
                for child in children {
                    rec(doc, engine, ctx, child, &style, map);
                }
            }
            NodeData::Text(_) => {
                // Text nodes inherit the full parent style for rendering.
                map.styles.insert(node, parent_style.clone());
            }
            _ => {
                let children = doc.node(node).children.clone();
                for child in children {
                    rec(doc, engine, ctx, child, parent_style, map);
                }
            }
        }
    }

    let initial = ComputedStyle {
        display: crate::values::Display::Block,
        font_size: ctx.root_font_size,
        ..ComputedStyle::default()
    };
    rec(doc, engine, ctx, root, &initial, &mut map);
    map
}

fn apply_inline(
    style: &mut ComputedStyle,
    css: &str,
    parent: Option<&ComputedStyle>,
    ctx: &CascadeCtx,
) {
    let leaked: &'static str = Box::leak(css.to_string().into_boxed_str());
    // Inline declarations never carry at-rules; simple block parse suffices.
    if let Ok(block) = lightningcss::stylesheet::StyleAttribute::parse(
        leaked,
        lightningcss::stylesheet::ParserOptions::default(),
    ) {
        let block: lightningcss::stylesheet::StyleAttribute<'static> =
            lightningcss::traits::IntoOwned::into_owned(block);
        for prop in &block.declarations.declarations {
            apply_property(style, prop, parent, ctx);
        }
        for prop in &block.declarations.important_declarations {
            apply_property(style, prop, parent, ctx);
        }
    }
}

/// Convenience: build a `StyleEngine` from author sheets plus the UA sheet.
impl StyleEngine {
    pub fn with_author_sheets(sheets: &[Stylesheet]) -> StyleEngine {
        let mut all = vec![Stylesheet::parse(crate::stylesheet::UA_RESET, Origin::UserAgent)
            .expect("UA stylesheet must parse")];
        all.extend(sheets.iter().cloned());
        StyleEngine::new(&all)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use brows12_html::parse_document;

    #[test]
    fn author_rules_override_ua() {
        let doc = parse_document("<html><body><p class=\"big\">x</p></body></html>");
        let author =
            Stylesheet::parse("p.big { font-size: 32px; color: red; }", Origin::Author).unwrap();
        let engine = StyleEngine::with_author_sheets(&[author]);
        let styles = compute_styles(&doc, &engine, &CascadeCtx::default());
        let p = doc.get_elements_by_tag_name("p")[0];
        let s = styles.get(p).unwrap();
        assert_eq!(s.font_size, 32.0);
        assert_eq!(s.color, [255, 0, 0, 255]);
        assert_eq!(s.display, crate::values::Display::Block); // UA default
    }

    #[test]
    fn specificity_beats_order() {
        let doc = parse_document("<html><body><div id=\"a\"><p>x</p></div></body></html>");
        let author =
            Stylesheet::parse("p { color: blue; } div#a p { color: green; }", Origin::Author)
                .unwrap();
        let engine = StyleEngine::with_author_sheets(&[author]);
        let styles = compute_styles(&doc, &engine, &CascadeCtx::default());
        let p = doc.get_elements_by_tag_name("p")[0];
        assert_eq!(styles.get(p).unwrap().color, [0, 128, 0, 255]);
    }

    #[test]
    fn inline_style_wins() {
        let doc = parse_document("<html><body><p style=\"color: purple\">x</p></body></html>");
        let author = Stylesheet::parse("p { color: blue; }", Origin::Author).unwrap();
        let engine = StyleEngine::with_author_sheets(&[author]);
        let styles = compute_styles(&doc, &engine, &CascadeCtx::default());
        let p = doc.get_elements_by_tag_name("p")[0];
        assert_eq!(styles.get(p).unwrap().color, [128, 0, 128, 255]);
    }

    #[test]
    fn inheritance_of_color_and_font() {
        let doc = parse_document("<html><body style=\"color: #ff0000\"><p>x</p></body></html>");
        let engine = StyleEngine::with_author_sheets(&[]);
        let styles = compute_styles(&doc, &engine, &CascadeCtx::default());
        let p = doc.get_elements_by_tag_name("p")[0];
        assert_eq!(styles.get(p).unwrap().color, [255, 0, 0, 255]);
    }

    #[test]
    fn display_none_hides_head() {
        let doc = parse_document("<html><head><title>t</title></head><body></body></html>");
        let engine = StyleEngine::with_author_sheets(&[]);
        let styles = compute_styles(&doc, &engine, &CascadeCtx::default());
        let title = doc.get_elements_by_tag_name("title")[0];
        assert_eq!(styles.get(title).unwrap().display, crate::values::Display::None);
    }
}
