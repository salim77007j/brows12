//! Cascade: match rules, order them, apply declarations, inherit.
//!
//! v0.2: at-rule aware. Rules are filtered through their enclosing
//! `@media` / `@supports` / `@container` context, cascade layers reorder
//! declarations, and `var()` custom-property substitution runs before
//! application. A second pass supports container queries with real sizes.

use crate::apply::apply_property;
use crate::atr::{container_condition_matches, media_list_matches, supports_matches, SupportCaps};
use crate::computed::{CascadeCtx, ComputedStyle};
use crate::matcher;
use crate::stylesheet::{Origin, StyleEngine, StyleRule, Stylesheet};
use crate::values::ContainerType;
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

/// Sizes of container ancestors from the previous layout pass
/// (node id -> inline size), used by the container-query pass.
pub type ContainerSizes = HashMap<NodeId, f32>;

/// Compute styles for every element (and text node) of `doc` using the
/// combined author + UA rule set of `engine`.
pub fn compute_styles(doc: &Document, engine: &StyleEngine, ctx: &CascadeCtx) -> StyleMap {
    compute_inner(doc, engine, ctx, &ContainerSizes::default(), None)
}

/// Container-query aware variant: run after a layout pass with the inline
/// sizes of `container-type` elements from that layout.
pub fn compute_styles_with_containers(
    doc: &Document,
    engine: &StyleEngine,
    ctx: &CascadeCtx,
    container_sizes: &ContainerSizes,
    previous: &StyleMap,
) -> StyleMap {
    compute_inner(doc, engine, ctx, container_sizes, Some(previous))
}

/// Does `rule` apply given the current device + container environment?
fn rule_applies(
    rule: &StyleRule,
    ctx: &CascadeCtx,
    doc: &Document,
    node: NodeId,
    container_sizes: &ContainerSizes,
    prev_styles: Option<&StyleMap>,
) -> bool {
    for list in &rule.media {
        if !media_list_matches(list, &ctx.device) {
            return false;
        }
    }
    for cond in &rule.supports {
        if !supports_matches(cond, &SupportCaps) {
            return false;
        }
    }
    if !rule.containers.is_empty() {
        // Walk up to the nearest container ancestor.
        let mut ancestor = doc.parent(node).filter(|&p| doc.is_element(p));
        let mut matched_any_container = false;
        while let Some(a) = ancestor {
            let c_type = prev_styles
                .and_then(|s| s.get(a))
                .map(|s| s.container_type)
                .unwrap_or(ContainerType::Normal);
            if c_type != ContainerType::Normal {
                matched_any_container = true;
                let size = container_sizes.get(&a).copied();
                let block = None; // block-size containers: not tracked yet
                if !rule.containers.iter().all(|c| container_condition_matches(c, size, block)) {
                    return false;
                }
                // Only the nearest container counts.
                break;
            }
            ancestor = doc.parent(a).filter(|&p| doc.is_element(p));
        }
        let _ = matched_any_container;
        // A rule inside @container with no resolvable container never matches.
        if !has_container_ancestor(doc, node, prev_styles) {
            return false;
        }
    }
    true
}

fn has_container_ancestor(doc: &Document, node: NodeId, prev_styles: Option<&StyleMap>) -> bool {
    let mut ancestor = doc.parent(node).filter(|&p| doc.is_element(p));
    while let Some(a) = ancestor {
        if let Some(s) = prev_styles.and_then(|s| s.get(a)) {
            if s.container_type != ContainerType::Normal {
                return true;
            }
        }
        ancestor = doc.parent(a).filter(|&p| doc.is_element(p));
    }
    false
}

/// Compute styles with an optional container environment.
#[allow(clippy::too_many_arguments)]
fn compute_inner(
    doc: &Document,
    engine: &StyleEngine,
    ctx: &CascadeCtx,
    container_sizes: &ContainerSizes,
    prev_styles: Option<&StyleMap>,
) -> StyleMap {
    let mut map = StyleMap::default();
    let root = doc.root();

    fn rec(
        doc: &Document,
        engine: &StyleEngine,
        ctx: &CascadeCtx,
        node: NodeId,
        parent_style: &ComputedStyle,
        map: &mut StyleMap,
        container_sizes: &ContainerSizes,
        prev_styles: Option<&StyleMap>,
    ) {
        let data = doc.node(node).data.clone();
        match &data {
            NodeData::Element { .. } => {
                let mut style = ComputedStyle::inherit_from(parent_style);
                style.font_size = parent_style.font_size;

                // Cascade matching rules that pass their at-rule context.
                let matched: Vec<&StyleRule> = engine
                    .rules()
                    .iter()
                    .filter(|rule| {
                        if !rule_applies(rule, ctx, doc, node, container_sizes, prev_styles) {
                            return false;
                        }
                        rule.selectors.0.iter().any(|sel| matcher::matches_selector(doc, node, sel))
                    })
                    .collect();

                // Layer-aware cascade order (see css-cascade-5).
                let layer_rank = |r: &StyleRule| engine.layers.rank_normal(&r.layer);
                let mut normal: Vec<&StyleRule> = matched.clone();
                normal.sort_by(|a, b| {
                    layer_rank(a)
                        .cmp(&layer_rank(b))
                        .then(a.specificity.cmp(&b.specificity))
                        .then(a.order.cmp(&b.order))
                });
                let mut important: Vec<&StyleRule> = matched;
                let ilayer_rank = |r: &StyleRule| engine.layers.rank_important(&r.layer);
                important.sort_by(|a, b| {
                    ilayer_rank(a)
                        .cmp(&ilayer_rank(b))
                        .then(a.specificity.cmp(&b.specificity))
                        .then(a.order.cmp(&b.order))
                });

                for rule in &normal {
                    for prop in &rule.declarations {
                        apply_property(&mut style, prop, Some(parent_style), ctx);
                    }
                }
                for rule in &important {
                    for prop in &rule.important {
                        apply_property(&mut style, prop, Some(parent_style), ctx);
                    }
                }

                // Inline style="" declarations win over all author rules.
                if let Some(inline) = doc.attr(node, "style") {
                    apply_inline(&mut style, inline, Some(parent_style), ctx, engine);
                }

                map.styles.insert(node, style.clone());

                let children = doc.node(node).children.clone();
                for child in children {
                    rec(doc, engine, ctx, child, &style, map, container_sizes, prev_styles);
                }
            }
            NodeData::Text(_) => {
                // Text nodes inherit the full parent style for rendering.
                map.styles.insert(node, parent_style.clone());
            }
            _ => {
                let children = doc.node(node).children.clone();
                for child in children {
                    rec(doc, engine, ctx, child, parent_style, map, container_sizes, prev_styles);
                }
            }
        }
    }

    let initial = ComputedStyle {
        display: crate::values::Display::Block,
        font_size: ctx.root_font_size,
        ..ComputedStyle::default()
    };
    rec(doc, engine, ctx, root, &initial, &mut map, container_sizes, prev_styles);
    map
}

/// Custom properties from stylesheet rules are registered into the element's
/// map (document-global scope approximation; inline declarations win).
fn apply_inline(
    style: &mut ComputedStyle,
    css: &str,
    parent: Option<&ComputedStyle>,
    ctx: &CascadeCtx,
    engine: &StyleEngine,
) {
    // Inline custom properties + inherited/global vars resolve at text level.
    let mut vars: HashMap<String, String> = HashMap::new();
    if let Some(p) = parent {
        vars.extend(p.custom.iter().map(|(k, v)| (k.clone(), v.clone())));
    }
    vars.extend(engine.global_vars.iter().map(|(k, v)| (k.clone(), v.clone())));
    crate::vartext::collect_custom_defs(css, &mut vars);
    let resolved = crate::vartext::substitute_vars_text(css, &vars);

    // Parse the substituted text as a borrowed block, then convert to an
    // owned 'static block. `IntoOwned::into_owned` copies every borrowed
    // token, so the parse input only needs to outlive the parse — leaking
    // the text here (as earlier revisions did) is unnecessary and would
    // grow per-element with every var()-bearing inline style.
    if let Ok(block) = lightningcss::stylesheet::StyleAttribute::parse(
        &resolved,
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
    // The raw declaration text may define --custom props descendants inherit.
    if css.contains("--") {
        let mut own = HashMap::new();
        crate::vartext::collect_custom_defs(css, &mut own);
        for (k, v) in own {
            style.custom.entry(k).or_insert(v);
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
    use crate::computed::CascadeCtx;
    use brows12_html::parse_document;

    fn author(css: &str) -> Stylesheet {
        Stylesheet::parse(css, Origin::Author).unwrap()
    }

    #[test]
    fn author_rules_override_ua() {
        let doc = parse_document("<html><body><p class=\"big\">x</p></body></html>");
        let engine =
            StyleEngine::with_author_sheets(&[author("p.big { font-size: 32px; color: red; }")]);
        let styles = compute_styles(&doc, &engine, &CascadeCtx::default());
        let p = doc.get_elements_by_tag_name("p")[0];
        let s = styles.get(p).unwrap();
        assert_eq!(s.font_size, 32.0);
        assert_eq!(s.color, [255, 0, 0, 255]);
        assert_eq!(s.display, crate::values::Display::Block); // UA default
    }

    #[test]
    fn media_query_filters_rules() {
        let doc = parse_document("<html><body><p>x</p></body></html>");
        let engine = StyleEngine::with_author_sheets(&[author(
            "@media (min-width: 900px) { p { color: red; } } @media (min-width: 2000px) { p { color: blue; } }",
        )]);
        let ctx = CascadeCtx { viewport_width: 1280.0, ..CascadeCtx::default() };
        let styles = compute_styles(&doc, &engine, &ctx);
        let p = doc.get_elements_by_tag_name("p")[0];
        assert_eq!(styles.get(p).unwrap().color, [255, 0, 0, 255]);
        // Narrow viewport: desktop rule must not apply.
        let ctx_narrow = CascadeCtx {
            viewport_width: 375.0,
            viewport_height: 700.0,
            device: crate::atr::DeviceEnv {
                viewport_width: 375.0,
                viewport_height: 700.0,
                ..crate::atr::DeviceEnv::default()
            },
            ..CascadeCtx::default()
        };
        let styles2 = compute_styles(&doc, &engine, &ctx_narrow);
        assert_eq!(styles2.get(p).unwrap().color, [0, 0, 0, 255]); // initial
    }

    #[test]
    fn custom_properties_and_var() {
        let doc = parse_document("<html><body><p>x</p></body></html>");
        let engine = StyleEngine::with_author_sheets(&[author(
            ":root { --accent: #ff0000; --size: 24px; } p { color: var(--accent); font-size: var(--size, 10px); }",
        )]);
        let styles = compute_styles(&doc, &engine, &CascadeCtx::default());
        let p = doc.get_elements_by_tag_name("p")[0];
        let s = styles.get(p).unwrap();
        assert_eq!(s.color, [255, 0, 0, 255], "var() color substitution");
        assert_eq!(s.font_size, 24.0, "var() length substitution");
    }

    #[test]
    fn var_fallback_and_missing() {
        let doc = parse_document("<html><body><p>x</p></body></html>");
        let engine = StyleEngine::with_author_sheets(&[author(
            "p { font-size: var(--missing, 20px); line-height: var(--also-missing); }",
        )]);
        let styles = compute_styles(&doc, &engine, &CascadeCtx::default());
        let p = doc.get_elements_by_tag_name("p")[0];
        let s = styles.get(p).unwrap();
        assert_eq!(s.font_size, 20.0, "fallback applies when var missing");
        assert_eq!(
            s.line_height,
            crate::values::LineHeight::Normal,
            "invalid var drops declaration"
        );
    }

    #[test]
    fn supports_filters_rules() {
        let doc = parse_document("<html><body><p>x</p></body></html>");
        let engine = StyleEngine::with_author_sheets(&[author(
            "@supports (display: flex) { p { color: red; } } @supports (display: totally-fake-display) { p { color: blue; } }",
        )]);
        let styles = compute_styles(&doc, &engine, &CascadeCtx::default());
        let p = doc.get_elements_by_tag_name("p")[0];
        assert_eq!(styles.get(p).unwrap().color, [255, 0, 0, 255]);
    }

    #[test]
    fn cascade_layers_order() {
        let doc = parse_document("<html><body><p>x</p></body></html>");
        // base wins over theme for normal declarations (declared first loses).
        let engine = StyleEngine::with_author_sheets(&[author(
            "@layer theme, base; @layer theme { p { color: blue; } } @layer base { p { color: green; } }",
        )]);
        let styles = compute_styles(&doc, &engine, &CascadeCtx::default());
        let p = doc.get_elements_by_tag_name("p")[0];
        assert_eq!(styles.get(p).unwrap().color, [0, 128, 0, 255]);
    }

    #[test]
    fn keyframes_collected() {
        let sheet = author(
            "@keyframes pulse { from { opacity: 1; } 50% { opacity: 0.5; } to { opacity: 0; } }",
        );
        assert!(sheet.keyframes.contains_key("pulse"));
        let kf = &sheet.keyframes["pulse"];
        assert_eq!(kf.frames.len(), 3);
        assert!((kf.frames[0].offset - 0.0).abs() < f32::EPSILON);
        assert!((kf.frames[1].offset - 0.5).abs() < f32::EPSILON);
        assert!((kf.frames[2].offset - 1.0).abs() < f32::EPSILON);
    }
}

// Re-export for downstream crates.
