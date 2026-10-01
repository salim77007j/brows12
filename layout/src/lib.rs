//! # brows12-layout
//!
//! Box layout for the Brows12 engine, built on `taffy` (the flexbox/grid/
//! block implementation powering Bevy and Dioxus) with text measurement
//! through `cosmic-text` (HarfBuzz-grade shaping via rustybuzz/swash).
//!
//! v1 supports block flow (margin/padding/border boxes), CSS flexbox
//! (direction, wrap, justify, align, gap) and wrapped text leaves.
//! Inline flow (mixed inline boxes on one line), floats and tables are
//! tracked in docs/ROADMAP.md.

use brows12_css::values::{AutoPx, Display, Len};
use brows12_css::ComputedStyle;
use brows12_html::{Document, NodeData, NodeId};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Viewport the page lays out into.
#[derive(Debug, Clone, Copy)]
pub struct Viewport {
    pub width: f32,
    pub height: f32,
}

impl Default for Viewport {
    fn default() -> Self {
        Viewport { width: 1280.0, height: 720.0 }
    }
}

/// Absolute-position rectangle in CSS pixels.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// Leaf context attached to taffy nodes that need measurement.
#[derive(Debug, Clone)]
pub enum LeafContext {
    Text {
        text: String,
        font_size: f32,
        line_height_px: f32,
        font_weight: u16,
        font_style_italic: bool,
        font_family: Option<String>,
        white_space_pre: bool,
    },
    Image {
        intrinsic_width: u32,
        intrinsic_height: u32,
    },
}

/// Result of a layout pass: absolute rects for every laid-out node.
#[derive(Debug, Clone, Default)]
pub struct LayoutResult {
    pub rects: HashMap<NodeId, Rect>,
    /// Total content height (for scrollbars / budgeting).
    pub content_height: f32,
    pub content_width: f32,
}

impl LayoutResult {
    pub fn rect(&self, node: NodeId) -> Option<Rect> {
        self.rects.get(&node).copied()
    }
}

/// Text measurement context shared with the rendering crate.
pub struct TextMeasurer {
    pub font_system: Arc<Mutex<cosmic_text::FontSystem>>,
}

impl TextMeasurer {
    pub fn new(font_system: Arc<Mutex<cosmic_text::FontSystem>>) -> Self {
        Self { font_system }
    }

    /// Measure `text` within `max_width` (None = max-content).
    /// Returns (used_width, total_height).
    pub fn measure(&self, leaf: &LeafContext, max_width: Option<f32>) -> (f32, f32) {
        match leaf {
            LeafContext::Image { intrinsic_width, intrinsic_height } => {
                let (w, h) = (*intrinsic_width as f32, *intrinsic_height as f32);
                (w, h)
            }
            LeafContext::Text {
                text,
                font_size,
                line_height_px,
                font_weight,
                font_style_italic,
                font_family,
                white_space_pre: _,
            } => {
                let mut fs = self.font_system.lock().unwrap();
                let metrics = cosmic_text::Metrics::new(*font_size, *line_height_px);
                let mut buffer = cosmic_text::Buffer::new(&mut fs, metrics);
                buffer.set_size(max_width, None);

                let mut attrs = cosmic_text::Attrs::new();
                attrs = match font_family.as_deref() {
                    Some("monospace") => attrs.family(cosmic_text::Family::Monospace),
                    Some("serif") => attrs.family(cosmic_text::Family::Serif),
                    Some(name) => attrs.family(cosmic_text::Family::Name(name)),
                    None => attrs.family(cosmic_text::Family::SansSerif),
                };
                attrs = attrs.weight(cosmic_text::Weight(*font_weight));
                if *font_style_italic {
                    attrs = attrs.style(cosmic_text::Style::Italic);
                }
                buffer.set_text(text, &attrs, cosmic_text::Shaping::Advanced, None);
                buffer.shape_until_scroll(&mut fs, false);

                let mut width = 0.0f32;
                let mut height = 0.0f32;
                for run in buffer.layout_runs() {
                    width = width.max(run.line_w);
                    height = run.line_top + run.line_height;
                }
                (width, height.max(*line_height_px))
            }
        }
    }
}

/// Map CSS values onto taffy styles.
fn taffy_dimension(v: AutoPx) -> taffy::Dimension {
    match v {
        AutoPx::Auto => taffy::Dimension::auto(),
        AutoPx::Len(Len::Px(px)) => taffy::Dimension::length(px),
        AutoPx::Len(Len::Percent(p)) => taffy::Dimension::percent(p / 100.0),
    }
}

fn taffy_len_or_percent(v: Len) -> taffy::LengthPercentage {
    match v {
        Len::Px(px) => taffy::LengthPercentage::length(px),
        Len::Percent(p) => taffy::LengthPercentage::percent(p / 100.0),
    }
}

fn taffy_auto_len(v: AutoPx) -> taffy::LengthPercentageAuto {
    match v {
        AutoPx::Auto => taffy::LengthPercentageAuto::auto(),
        AutoPx::Len(Len::Px(px)) => taffy::LengthPercentageAuto::length(px),
        AutoPx::Len(Len::Percent(p)) => taffy::LengthPercentageAuto::percent(p / 100.0),
    }
}

fn build_taffy_style(style: &ComputedStyle) -> taffy::Style {
    let display = match style.display {
        Display::None => taffy::Display::None,
        Display::Flex => taffy::Display::Flex,
        Display::Block | Display::Inline => taffy::Display::Block,
    };
    let mut taffy_style = taffy::Style {
        display,
        size: taffy::Size {
            width: taffy_dimension(style.width),
            height: taffy_dimension(style.height),
        },
        margin: taffy::Rect {
            top: taffy_auto_len(style.margin.top),
            right: taffy_auto_len(style.margin.right),
            bottom: taffy_auto_len(style.margin.bottom),
            left: taffy_auto_len(style.margin.left),
        },
        padding: taffy::Rect {
            top: taffy_len_or_percent(style.padding.top),
            right: taffy_len_or_percent(style.padding.right),
            bottom: taffy_len_or_percent(style.padding.bottom),
            left: taffy_len_or_percent(style.padding.left),
        },
        border: taffy::Rect {
            top: taffy_len_or_percent(style.border_width.top),
            right: taffy_len_or_percent(style.border_width.right),
            bottom: taffy_len_or_percent(style.border_width.bottom),
            left: taffy_len_or_percent(style.border_width.left),
        },
        gap: taffy::Size {
            width: taffy_len_or_percent(style.column_gap),
            height: taffy_len_or_percent(style.row_gap),
        },
        ..taffy::Style::default()
    };

    if style.display == Display::Flex {
        taffy_style.flex_direction = match style.flex_direction {
            brows12_css::values::FlexDirection::Row => taffy::FlexDirection::Row,
            brows12_css::values::FlexDirection::RowReverse => taffy::FlexDirection::RowReverse,
            brows12_css::values::FlexDirection::Column => taffy::FlexDirection::Column,
            brows12_css::values::FlexDirection::ColumnReverse => {
                taffy::FlexDirection::ColumnReverse
            }
        };
        taffy_style.flex_wrap = match style.flex_wrap {
            brows12_css::values::FlexWrap::NoWrap => taffy::FlexWrap::NoWrap,
            brows12_css::values::FlexWrap::Wrap => taffy::FlexWrap::Wrap,
            brows12_css::values::FlexWrap::WrapReverse => taffy::FlexWrap::WrapReverse,
        };
        taffy_style.justify_content = Some(match style.justify_content {
            brows12_css::values::JustifyContent::FlexStart => taffy::AlignContent::FLEX_START,
            brows12_css::values::JustifyContent::FlexEnd => taffy::AlignContent::FLEX_END,
            brows12_css::values::JustifyContent::Center => taffy::AlignContent::CENTER,
            brows12_css::values::JustifyContent::SpaceBetween => taffy::AlignContent::SPACE_BETWEEN,
            brows12_css::values::JustifyContent::SpaceAround => taffy::AlignContent::SPACE_AROUND,
            brows12_css::values::JustifyContent::SpaceEvenly => taffy::AlignContent::SPACE_EVENLY,
            brows12_css::values::JustifyContent::Start => taffy::AlignContent::START,
            brows12_css::values::JustifyContent::End => taffy::AlignContent::END,
        });
        taffy_style.align_items = Some(match style.align_items {
            brows12_css::values::AlignItems::Stretch => taffy::AlignItems::STRETCH,
            brows12_css::values::AlignItems::FlexStart => taffy::AlignItems::FLEX_START,
            brows12_css::values::AlignItems::FlexEnd => taffy::AlignItems::FLEX_END,
            brows12_css::values::AlignItems::Center => taffy::AlignItems::CENTER,
            brows12_css::values::AlignItems::Baseline => taffy::AlignItems::BASELINE,
            brows12_css::values::AlignItems::Start => taffy::AlignItems::START,
            brows12_css::values::AlignItems::End => taffy::AlignItems::END,
        });
        taffy_style.flex_grow = style.flex.grow;
        taffy_style.flex_shrink = style.flex.shrink;
    }
    taffy_style
}

fn leaf_context(style: &ComputedStyle, text: &str) -> LeafContext {
    LeafContext::Text {
        text: text.to_string(),
        font_size: style.font_size,
        line_height_px: style.line_height_px(),
        font_weight: style.font_weight,
        font_style_italic: style.font_style == brows12_css::values::FontStyle::Italic,
        font_family: style.font_family.clone(),
        white_space_pre: style.white_space == brows12_css::values::WhiteSpace::Pre,
    }
}

/// Build the taffy tree + compute layout in one pass.
pub fn compute_layout(
    doc: &Document,
    styles: &brows12_css::StyleMap,
    viewport: Viewport,
    measurer: &TextMeasurer,
    image_sizes: &HashMap<NodeId, (u32, u32)>,
) -> LayoutResult {
    let mut tree: taffy::TaffyTree<LeafContext> = taffy::TaffyTree::new();
    let mut node_ids: HashMap<NodeId, taffy::NodeId> = HashMap::new();

    // Start at body when present so the root box matches the viewport.
    let start = doc.body().or_else(|| doc.document_element()).unwrap_or(doc.root());

    fn build(
        doc: &Document,
        styles: &brows12_css::StyleMap,
        tree: &mut taffy::TaffyTree<LeafContext>,
        node_ids: &mut HashMap<NodeId, taffy::NodeId>,
        image_sizes: &HashMap<NodeId, (u32, u32)>,
        node: NodeId,
    ) -> Option<taffy::NodeId> {
        let style = styles.get(node)?;
        if style.display == Display::None {
            return None;
        }
        let data = doc.node(node).data.clone();
        match &data {
            NodeData::Text(text) => {
                if text.trim().is_empty() {
                    return None;
                }
                let ctx = leaf_context(style, text);
                let tnode = tree.new_leaf_with_context(build_taffy_style(style), ctx).ok()?;
                node_ids.insert(node, tnode);
                Some(tnode)
            }
            NodeData::Element { .. } => {
                let taffy_style = build_taffy_style(style);
                let children: Vec<taffy::NodeId> = doc
                    .node(node)
                    .children
                    .iter()
                    .filter_map(|&c| build(doc, styles, tree, node_ids, image_sizes, c))
                    .collect();

                // Leaf elements with intrinsic size (img).
                if children.is_empty() {
                    if let Some(&(w, h)) = image_sizes.get(&node) {
                        let ctx = LeafContext::Image { intrinsic_width: w, intrinsic_height: h };
                        let tnode = tree.new_leaf_with_context(taffy_style, ctx).ok()?;
                        node_ids.insert(node, tnode);
                        return Some(tnode);
                    }
                }

                let tnode = if children.is_empty() {
                    tree.new_leaf(taffy_style).ok()?
                } else {
                    let tnode = tree.new_with_children(taffy_style, &children).ok()?;
                    node_ids.insert(node, tnode);
                    return Some(tnode);
                };
                node_ids.insert(node, tnode);
                Some(tnode)
            }
            _ => {
                // Document/doctype/comments: recurse through children.
                for &c in &doc.node(node).children {
                    if let Some(t) = build(doc, styles, tree, node_ids, image_sizes, c) {
                        return Some(t);
                    }
                }
                None
            }
        }
    }

    let Some(root_taffy) = build(doc, styles, &mut tree, &mut node_ids, image_sizes, start) else {
        return LayoutResult::default();
    };

    // Collapse a single html>body chain so body fills the viewport root.
    let layout_root = root_taffy;
    let measurer_ref = &measurer;
    let measure_fn = |input: taffy::LayoutInput,
                      _node: taffy::NodeId,
                      ctx: Option<&mut LeafContext>,
                      _style: &taffy::Style|
     -> taffy::LayoutOutput {
        if let Some(leaf) = ctx {
            let max_width = input.known_dimensions.width.or(match input.available_space.width {
                taffy::AvailableSpace::Definite(w) => Some(w),
                _ => None,
            });
            let (w, h) = measurer_ref.measure(leaf, max_width);
            let size = taffy::Size { width: w, height: h };
            return taffy::LayoutOutput::from_sizes(
                size,
                taffy::Rect { left: 0.0, right: w, top: 0.0, bottom: h },
            );
        }
        taffy::LayoutOutput::from_sizes(
            taffy::Size { width: 0.0, height: 0.0 },
            taffy::Rect { left: 0.0, right: 0.0, top: 0.0, bottom: 0.0 },
        )
    };

    let _ = tree.compute_layout_with_measure(
        layout_root,
        taffy::Size {
            width: taffy::AvailableSpace::Definite(viewport.width),
            height: taffy::AvailableSpace::Definite(viewport.height),
        },
        measure_fn,
    );

    // Extract absolute rects via depth-first accumulation.
    let mut result = LayoutResult::default();
    let taffy_to_dom: HashMap<taffy::NodeId, NodeId> =
        node_ids.iter().map(|(d, t)| (*t, *d)).collect();
    fn extract(
        taffy_to_dom: &HashMap<taffy::NodeId, NodeId>,
        tree: &taffy::TaffyTree<LeafContext>,
        rects: &mut HashMap<NodeId, Rect>,
        taffy_node: taffy::NodeId,
        offset: (f32, f32),
    ) {
        let Ok(layout) = tree.layout(taffy_node) else {
            return;
        };
        let x = offset.0 + layout.location.x;
        let y = offset.1 + layout.location.y;
        let rect = Rect { x, y, width: layout.size.width, height: layout.size.height };
        if let Some(dom_id) = taffy_to_dom.get(&taffy_node) {
            rects.insert(*dom_id, rect);
        }
        if let Ok(children) = tree.children(taffy_node) {
            for child in children {
                extract(taffy_to_dom, tree, rects, child, (x, y));
            }
        }
    }
    extract(&taffy_to_dom, &tree, &mut result.rects, root_taffy, (0.0, 0.0));

    if let Some(body_rect) = doc.body().and_then(|b| result.rects.get(&b).copied()) {
        result.content_height = body_rect.y + body_rect.height;
        result.content_width = viewport.width;
    } else {
        result.content_height = viewport.height;
        result.content_width = viewport.width;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use brows12_css::computed::CascadeCtx;
    use brows12_css::{compute_styles, StyleEngine};
    use brows12_html::parse_document;

    fn test_measurer() -> TextMeasurer {
        TextMeasurer::new(Arc::new(Mutex::new(cosmic_text::FontSystem::new())))
    }

    fn layout_html(html: &str, viewport: Viewport) -> (Document, LayoutResult) {
        let doc = parse_document(html);
        let engine = StyleEngine::with_author_sheets(&[]);
        let ctx = CascadeCtx {
            viewport_width: viewport.width,
            viewport_height: viewport.height,
            ..CascadeCtx::default()
        };
        let styles = compute_styles(&doc, &engine, &ctx);
        let measurer = test_measurer();
        let result = compute_layout(&doc, &styles, viewport, &measurer, &HashMap::new());
        (doc, result)
    }

    #[test]
    fn block_stacking() {
        let (doc, result) = layout_html(
            "<html><body><p>a</p><p>b</p></body></html>",
            Viewport { width: 800.0, height: 600.0 },
        );
        let ps = doc.get_elements_by_tag_name("p");
        let a = result.rect(ps[0]).unwrap();
        let b = result.rect(ps[1]).unwrap();
        assert!(a.height > 0.0, "p must have content height");
        assert!(b.y >= a.y + a.height - 1.0, "blocks stack vertically");
    }

    #[test]
    fn flexbox_row() {
        let html = "<html><body><div style=\"display:flex; flex-direction:row;\"><span style=\"width:100px\">x</span><span style=\"width:100px\">y</span></div></body></html>";
        let (doc, result) = layout_html(html, Viewport { width: 800.0, height: 600.0 });
        let spans = doc.get_elements_by_tag_name("span");
        let s0 = result.rect(spans[0]).unwrap();
        let s1 = result.rect(spans[1]).unwrap();
        assert!(s1.x > s0.x + 50.0, "flex row places items side by side ({} vs {})", s0.x, s1.x);
    }

    #[test]
    fn padding_applies() {
        // NOTE: margin-top would collapse with the body margin per CSS, so we
        // assert on padding, which always moves the text down.
        let (doc, result) = layout_html(
            "<html><body><div style=\"padding-top:50px\">x</div></body></html>",
            Viewport { width: 800.0, height: 600.0 },
        );
        let div = doc.get_elements_by_tag_name("div")[0];
        let r = result.rect(div).unwrap();
        assert!(r.height >= 50.0, "padding-top adds box height, got {}", r.height);
    }

    #[test]
    fn text_wraps_to_multiple_lines() {
        let html = "<html><body><p style=\"width:120px\">aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa bbbbbbbbbbbbbbbbbbbbbbbbbb cccccccccccc</p></body></html>";
        let (_doc, result) = layout_html(html, Viewport { width: 800.0, height: 600.0 });
        // single unwrapped line at 16px font would be ~500px wide
        let ps = _doc.get_elements_by_tag_name("p");
        let r = result.rect(ps[0]).unwrap();
        assert!(r.height > 30.0, "wrapped text should be taller than one line, got {}", r.height);
    }

    #[test]
    fn display_none_is_removed() {
        let (doc, result) = layout_html(
            "<html><body><div style=\"display:none\">hidden</div><div>visible</div></body></html>",
            Viewport { width: 800.0, height: 600.0 },
        );
        let divs = doc.get_elements_by_tag_name("div");
        assert!(result.rect(divs[0]).is_none());
        assert!(result.rect(divs[1]).is_some());
    }
}
