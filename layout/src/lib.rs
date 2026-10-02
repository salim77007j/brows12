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

use brows12_css::values::{AutoPx, Display, Len, Position as CssPosition, TextAlign};
use brows12_css::ComputedStyle;
use brows12_html::{Document, NodeData, NodeId};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

pub mod inline;
pub use inline::{
    apply_alignment, collapse_ws, InlineFlowLayout, InlineItem, InlineLine, InlineSegment,
};

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
    /// An inline group: styled runs from nested inline elements flowed
    /// across line boxes by our own line-breaking engine.
    Inline {
        items: Arc<Vec<InlineItem>>,
        align: TextAlign,
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
    /// Nodes with `position: fixed` — compositor promotes these to
    /// viewport-anchored layers (they do not scroll).
    pub fixed_nodes: Vec<NodeId>,
    /// Per-node transform (translate/rotate/scale) from CSS `transform`.
    pub transforms: HashMap<NodeId, brows12_css::values::Transform>,
    /// Laid-out inline flows keyed by the first member node of each group.
    pub inline_flows: HashMap<NodeId, InlineFlowLayout>,
    /// Nodes painted as part of an inline flow (must not emit their own
    /// display items). Includes every member except each group's first.
    pub inline_covered: HashSet<NodeId>,
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
            LeafContext::Inline { items, .. } => {
                let flow = inline::layout_inline(self, items, max_width);
                (flow.width, flow.height.max(0.0))
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
        AutoPx::Len(Len::Percent(p)) => taffy::Dimension::percent(p),
    }
}

fn taffy_len_or_percent(v: Len) -> taffy::LengthPercentage {
    match v {
        Len::Px(px) => taffy::LengthPercentage::length(px),
        Len::Percent(p) => taffy::LengthPercentage::percent(p),
    }
}

fn taffy_auto_len(v: AutoPx) -> taffy::LengthPercentageAuto {
    match v {
        AutoPx::Auto => taffy::LengthPercentageAuto::auto(),
        AutoPx::Len(Len::Px(px)) => taffy::LengthPercentageAuto::length(px),
        AutoPx::Len(Len::Percent(p)) => taffy::LengthPercentageAuto::percent(p),
    }
}

fn taffy_position(style: &ComputedStyle) -> taffy::Position {
    match style.position {
        CssPosition::Static => taffy::Position::Relative, // taffy default is Relative
        CssPosition::Relative => taffy::Position::Relative,
        // taffy positions absolute children against their parent box; the
        // post-pass below re-anchors `fixed` to the viewport.
        CssPosition::Absolute | CssPosition::Fixed | CssPosition::Sticky => {
            taffy::Position::Absolute
        }
    }
}

fn taffy_inset(style: &ComputedStyle) -> taffy::Rect<taffy::LengthPercentageAuto> {
    let f = |v: &AutoPx| match v {
        AutoPx::Auto => taffy::LengthPercentageAuto::auto(),
        AutoPx::Len(Len::Px(px)) => taffy::LengthPercentageAuto::length(*px),
        AutoPx::Len(Len::Percent(p)) => taffy::LengthPercentageAuto::percent(*p),
    };
    taffy::Rect {
        top: f(&style.insets.top),
        right: f(&style.insets.right),
        bottom: f(&style.insets.bottom),
        left: f(&style.insets.left),
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
        position: taffy_position(style),
        inset: taffy_inset(style),
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
        inline_ctx: &mut HashMap<NodeId, (Arc<Vec<InlineItem>>, TextAlign)>,
        covered: &mut HashSet<NodeId>,
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
                // Leaf elements with intrinsic size (img).
                if doc.node(node).children.is_empty() {
                    if let Some(&(w, h)) = image_sizes.get(&node) {
                        let taffy_style = build_taffy_style(style);
                        let ctx = LeafContext::Image { intrinsic_width: w, intrinsic_height: h };
                        let tnode = tree.new_leaf_with_context(taffy_style, ctx).ok()?;
                        node_ids.insert(node, tnode);
                        return Some(tnode);
                    }
                }

                let taffy_style = build_taffy_style(style);
                // Partition children into inline runs and block children
                // (CSS anonymous block boxes).
                let pieces = group_children(doc, styles, image_sizes, node);
                let mut children: Vec<taffy::NodeId> = Vec::new();
                for piece in pieces {
                    match piece {
                        Piece::Block(c) => {
                            if let Some(t) = build(
                                doc, styles, tree, node_ids, image_sizes, inline_ctx, covered, c,
                            ) {
                                children.push(t);
                            }
                        }
                        Piece::Inline(members) => {
                            if let Some(t) = build_inline_group(
                                doc, styles, tree, node_ids, image_sizes, inline_ctx, covered,
                                node, &members,
                            ) {
                                children.push(t);
                            }
                        }
                    }
                }

                let tnode = if children.is_empty() {
                    tree.new_leaf(taffy_style).ok()?
                } else {
                    tree.new_with_children(taffy_style, &children).ok()?
                };
                node_ids.insert(node, tnode);
                Some(tnode)
            }
            _ => {
                // Document/doctype/comments: recurse through children.
                for &c in &doc.node(node).children {
                    if let Some(t) =
                        build(doc, styles, tree, node_ids, image_sizes, inline_ctx, covered, c)
                    {
                        return Some(t);
                    }
                }
                None
            }
        }
    }

    /// One child partition: either a run of inline members (flattened into
    /// one anonymous inline box) or a single block-level child.
    enum Piece {
        Inline(Vec<NodeId>),
        Block(NodeId),
    }

    fn group_children(
        doc: &Document,
        styles: &brows12_css::StyleMap,
        image_sizes: &HashMap<NodeId, (u32, u32)>,
        node: NodeId,
    ) -> Vec<Piece> {
        // Flex items are block-ified per CSS — no inline runs inside flex
        // containers (each child is its own flex item).
        if styles.get(node).map(|s| s.display) == Some(Display::Flex) {
            return doc.node(node).children.iter().map(|&c| Piece::Block(c)).collect();
        }
        let mut pieces: Vec<Piece> = Vec::new();
        let mut cur: Vec<NodeId> = Vec::new();
        for &c in &doc.node(node).children {
            if is_inline_member(doc, styles, image_sizes, c) {
                cur.push(c);
            } else {
                if !cur.is_empty() {
                    pieces.push(Piece::Inline(std::mem::take(&mut cur)));
                }
                pieces.push(Piece::Block(c));
            }
        }
        if !cur.is_empty() {
            pieces.push(Piece::Inline(cur));
        }
        pieces
    }

    /// A node joins an inline run when it is a text node or an inline
    /// element whose whole subtree stays inline (no blocks, no atomics).
    fn is_inline_member(
        doc: &Document,
        styles: &brows12_css::StyleMap,
        image_sizes: &HashMap<NodeId, (u32, u32)>,
        node: NodeId,
    ) -> bool {
        match &doc.node(node).data {
            NodeData::Text(_) => true,
            NodeData::Element { .. } => {
                let Some(st) = styles.get(node) else { return false };
                st.display == Display::Inline
                    && !image_sizes.contains_key(&node)
                    && !contains_boundary(doc, styles, image_sizes, node)
            }
            _ => false,
        }
    }

    fn contains_boundary(
        doc: &Document,
        styles: &brows12_css::StyleMap,
        image_sizes: &HashMap<NodeId, (u32, u32)>,
        node: NodeId,
    ) -> bool {
        for &c in &doc.node(node).children {
            match &doc.node(c).data {
                NodeData::Text(_) => continue,
                NodeData::Element { .. } => {
                    let Some(st) = styles.get(c) else { return true };
                    if st.display == Display::None {
                        continue;
                    }
                    if st.display != Display::Inline || image_sizes.contains_key(&c) {
                        return true;
                    }
                    if contains_boundary(doc, styles, image_sizes, c) {
                        return true;
                    }
                }
                _ => {}
            }
        }
        false
    }

    /// Flatten inline members into styled items + create the anonymous
    /// taffy leaf that measures/holds the whole run. The group's rect is
    /// registered under its FIRST member so the display list can find it.
    #[allow(clippy::too_many_arguments)]
    fn build_inline_group(
        doc: &Document,
        styles: &brows12_css::StyleMap,
        tree: &mut taffy::TaffyTree<LeafContext>,
        node_ids: &mut HashMap<NodeId, taffy::NodeId>,
        _image_sizes: &HashMap<NodeId, (u32, u32)>,
        inline_ctx: &mut HashMap<NodeId, (Arc<Vec<InlineItem>>, TextAlign)>,
        covered: &mut HashSet<NodeId>,
        container: NodeId,
        members: &[NodeId],
    ) -> Option<taffy::NodeId> {
        let mut items: Vec<InlineItem> = Vec::new();
        for (i, &m) in members.iter().enumerate() {
            let mut visited: Vec<NodeId> = Vec::new();
            flatten_group(doc, styles, m, false, false, None, &mut items, &mut visited);
            if i == 0 {
                // First member roots the group (kept out of `covered` so the
                // display list can key on it); its descendants are covered.
                for &n in visited.iter().skip(1) {
                    covered.insert(n);
                }
            } else {
                for n in visited {
                    covered.insert(n);
                }
            }
        }
        // CSS whitespace processing at flow boundaries.
        if let Some(first) = items.first_mut() {
            if first.white_space != brows12_css::values::WhiteSpace::Pre {
                first.text = first.text.trim_start().to_string();
            }
        }
        if let Some(last) = items.last_mut() {
            if last.white_space != brows12_css::values::WhiteSpace::Pre {
                last.text = last.text.trim_end().to_string();
            }
        }
        items.retain(|it| !it.text.is_empty());
        if std::env::var("BROWS_DEBUG").is_ok() {
            eprintln!(
                "INLINE_GROUP container={container:?} members={} items={:?}",
                members.len(),
                items.iter().map(|i| i.text.as_str()).collect::<Vec<_>>()
            );
        }
        if items.is_empty() {
            return None; // whitespace-only run between blocks: no box
        }

        let container_style = styles.get(container)?;
        let align = container_style.text_align;
        let arc: Arc<Vec<InlineItem>> = Arc::new(items);

        let tnode = tree
            .new_leaf_with_context(
                taffy::Style::default(), // anonymous inline box: no margin/border
                LeafContext::Inline { items: arc.clone(), align },
            )
            .ok()?;
        node_ids.insert(members[0], tnode);
        inline_ctx.insert(members[0], (arc, align));
        Some(tnode)
    }

    /// Walk an inline subtree in tree order, emitting one `InlineItem` per
    /// text chunk with its computed style (accumulating decorations and
    /// inline backgrounds from ancestors).
    #[allow(clippy::too_many_arguments)]
    fn flatten_group(
        doc: &Document,
        styles: &brows12_css::StyleMap,
        node: NodeId,
        underline: bool,
        strike: bool,
        bg: Option<brows12_css::values::Rgba>,
        items: &mut Vec<InlineItem>,
        visited: &mut Vec<NodeId>,
    ) {
        visited.push(node);
        let Some(style) = styles.get(node) else { return };
        if style.display == Display::None {
            return;
        }
        match &doc.node(node).data {
            NodeData::Text(text) => {
                let pre = style.white_space == brows12_css::values::WhiteSpace::Pre;
                let text = if pre { text.clone() } else { collapse_ws(text) };
                if text.is_empty() {
                    return;
                }
                items.push(InlineItem {
                    text,
                    font_size: style.font_size,
                    line_height_px: style.line_height_px(),
                    font_weight: style.font_weight,
                    italic: style.font_style == brows12_css::values::FontStyle::Italic,
                    font_family: style.font_family.clone(),
                    color: style.color,
                    underline: underline || style.text_underline,
                    line_through: strike || style.text_line_through,
                    background: if style.background_color[3] > 0 {
                        Some(style.background_color)
                    } else {
                        bg
                    },
                    white_space: style.white_space,
                });
            }
            NodeData::Element { name, .. } => {
                if name == "br" {
                    items.push(InlineItem::newline(
                        style.font_size,
                        style.line_height_px(),
                        style,
                    ));
                    return;
                }
                let bg2 = if style.background_color[3] > 0 {
                    Some(style.background_color)
                } else {
                    bg
                };
                let und = underline || style.text_underline;
                let stk = strike || style.text_line_through;
                for &c in &doc.node(node).children {
                    flatten_group(doc, styles, c, und, stk, bg2, items, visited);
                }
            }
            _ => {}
        }
    }

    let mut inline_ctx: HashMap<NodeId, (Arc<Vec<InlineItem>>, TextAlign)> = HashMap::new();
    let mut covered: HashSet<NodeId> = HashSet::new();
    let Some(root_taffy) = build(
        doc,
        styles,
        &mut tree,
        &mut node_ids,
        image_sizes,
        &mut inline_ctx,
        &mut covered,
        start,
    ) else {
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
            if std::env::var("BROWS_DEBUG").is_ok() {
                eprintln!("EXTRACT: layout() FAILED for taffy={taffy_node:?}");
            }
            return;
        };
        if std::env::var("BROWS_DEBUG").is_ok() {
            if let Some(dom) = taffy_to_dom.get(&taffy_node) {
                eprintln!(
                    "EXTRACT taffy={taffy_node:?} dom={dom:?} at ({:.0},{:.0} {}x{})",
                    offset.0 + layout.location.x,
                    offset.1 + layout.location.y,
                    layout.size.width,
                    layout.size.height
                );
            }
        }
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
    if std::env::var("BROWS_DEBUG").is_ok() {
        eprintln!("LAYOUT node_ids: {} entries", node_ids.len());
        for (dom, taf) in &node_ids {
            let label = match &doc.node(*dom).data {
                NodeData::Element { name, .. } => format!("<{name}>"),
                NodeData::Text(t) => format!("text({:.14})", t.trim()),
                _ => "?".into(),
            };
            eprintln!("  dom={dom:?} {label} -> taffy={taf:?}");
        }
    }
    extract(&taffy_to_dom, &tree, &mut result.rects, root_taffy, (0.0, 0.0));

    // ---- Inline flows: final shape at each group's resolved box width ----
    for (dom_id, (items, align)) in inline_ctx.iter() {
        let Some(&tn) = node_ids.get(dom_id) else { continue };
        let w = match tree.layout(tn) {
            Ok(l) => l.size.width,
            Err(_) => continue,
        };
        let mut flow = inline::layout_inline(measurer, items, Some(w));
        apply_alignment(&mut flow, w, *align);
        result.inline_flows.insert(*dom_id, flow);
    }
    result.inline_covered = covered;

    // ---- v0.2 post-pass: fixed anchoring + transforms + content bounds ----
    // Children map for subtree shifting.
    let mut children_of: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
    let mut all_elements: Vec<NodeId> = Vec::new();
    fn collect_children(
        doc: &Document,
        node: NodeId,
        children_of: &mut HashMap<NodeId, Vec<NodeId>>,
        all: &mut Vec<NodeId>,
    ) {
        for &c in &doc.node(node).children {
            all.push(c);
            collect_children(doc, c, children_of, all);
            children_of.entry(node).or_default().push(c);
        }
    }
    collect_children(doc, start, &mut children_of, &mut all_elements);

    // Shift every descendant of `node` by `dx`/`dy` (recursive, pre-order).
    fn shift_subtree(
        rects: &mut HashMap<NodeId, Rect>,
        children_of: &HashMap<NodeId, Vec<NodeId>>,
        node: NodeId,
        dx: f32,
        dy: f32,
        depth: usize,
    ) {
        if depth > 256 {
            return;
        }
        if let Some(r) = rects.get_mut(&node) {
            r.x += dx;
            r.y += dy;
        }
        if let Some(kids) = children_of.get(&node) {
            for &k in kids {
                shift_subtree(rects, children_of, k, dx, dy, depth + 1);
            }
        }
    }

    // position: absolute — resolve insets ourselves against the parent's
    // padding box (taffy does not resolve percentage top/bottom insets on
    // absolute children). Auto top+bottom keep the STATIC flow position
    // (CSS 2.1 §10.3.7); same for auto left+right on the x axis.
    {
        // Parent map (walk from the layout start node).
        let mut parent_of: HashMap<NodeId, NodeId> = HashMap::new();
        fn collect_parents(
            doc: &Document,
            node: NodeId,
            parent_of: &mut HashMap<NodeId, NodeId>,
        ) {
            for &c in &doc.node(node).children {
                parent_of.insert(c, node);
                collect_parents(doc, c, parent_of);
            }
        }
        collect_parents(doc, start, &mut parent_of);

        let absolutes: Vec<NodeId> = all_elements
            .iter()
            .copied()
            .filter(|&n| {
                doc.is_element(n)
                    && styles.get(n).map(|s| s.position)
                        == Some(brows12_css::values::Position::Absolute)
            })
            .collect();
        for n in absolutes {
            let Some(style) = styles.get(n) else { continue };
            if std::env::var("BROWS_DEBUG").is_ok() {
                eprintln!(
                    "ABS n={n:?} pos={:?} insets=({:?},{:?},{:?},{:?})",
                    style.position,
                    style.insets.top,
                    style.insets.right,
                    style.insets.bottom,
                    style.insets.left
                );
            }
            let Some(&parent) = parent_of.get(&n) else { continue };
            let (Some(p_style), Some(p_rect)) =
                (styles.get(parent), result.rects.get(&parent).copied())
            else {
                continue;
            };
            let Some(r) = result.rects.get(&n).copied() else { continue };

            // Containing block: parent's padding box.
            let bl = len_px(&p_style.border_width.left);
            let bt = len_px(&p_style.border_width.top);
            let br = len_px(&p_style.border_width.right);
            let bb = len_px(&p_style.border_width.bottom);
            let cb_x = p_rect.x + bl;
            let cb_y = p_rect.y + bt;
            let cb_w = (p_rect.width - bl - br).max(0.0);
            let cb_h = (p_rect.height - bt - bb).max(0.0);

            let mt = offset_for_margin(style.margin.top);
            let mb = offset_for_margin(style.margin.bottom);
            let ml = offset_for_margin(style.margin.left);
            let mr = offset_for_margin(style.margin.right);

            // ---- X axis ----
            let nx = match (&style.insets.left, &style.insets.right) {
                (AutoPx::Len(_), _) => cb_x + inset_px(style.insets.left, cb_w).unwrap_or(0.0) + ml,
                (_, AutoPx::Len(_)) => {
                    cb_x + cb_w - inset_px(style.insets.right, cb_w).unwrap_or(0.0) - r.width - mr
                }
                _ => {
                    // Static position: where the box would sit in flow.
                    let mut cursor_x = cb_x + len_px(&p_style.padding.left);
                    for &c in &doc.node(parent).children {
                        if c == n {
                            cursor_x += ml;
                            break;
                        }
                        // Inline flow siblings: x stays at content left.
                    }
                    cursor_x
                }
            };

            // ---- Y axis ----
            let ny = match (&style.insets.top, &style.insets.bottom) {
                (AutoPx::Len(_), _) => cb_y + inset_px(style.insets.top, cb_h).unwrap_or(0.0) + mt,
                (_, AutoPx::Len(_)) => {
                    cb_y + cb_h - inset_px(style.insets.bottom, cb_h).unwrap_or(0.0) - r.height - mb
                }
                _ => {
                    // Static position: after the previous in-flow siblings.
                    let mut cursor_y = cb_y + len_px(&p_style.padding.top);
                    for &c in &doc.node(parent).children {
                        if c == n {
                            cursor_y += mt;
                            break;
                        }
                        if styles.get(c).map(|s| s.display) == Some(Display::None) {
                            continue;
                        }
                        if let Some(cr) = result.rects.get(&c) {
                            let cmb = styles
                                .get(c)
                                .map(|s| offset_for_margin(s.margin.bottom))
                                .unwrap_or(0.0);
                            cursor_y = cursor_y.max(cr.y + cr.height + cmb);
                        }
                    }
                    cursor_y
                }
            };

            let dx = nx - r.x;
            let dy = ny - r.y;
            if std::env::var("BROWS_DEBUG").is_ok() {
                eprintln!(
                    "ABS-> n={n:?} parent={parent:?} p_rect=({:.0},{:.0} {}x{}) cb=({:.0},{:.0} {}x{}) r=({:.0},{:.0}) -> ({:.0},{:.0})",
                    p_rect.x, p_rect.y, p_rect.width, p_rect.height,
                    cb_x, cb_y, cb_w, cb_h, r.x, r.y, nx, ny
                );
            }
            if dx != 0.0 || dy != 0.0 {
                if let Some(rr) = result.rects.get_mut(&n) {
                    rr.x = nx;
                    rr.y = ny;
                }
                shift_subtree(&mut result.rects, &children_of, n, dx, dy, 0);
            }
        }
    }

    // position: fixed — anchor to the viewport (initial containing block),
    // and record for compositor layer promotion.
    let fixed: Vec<NodeId> = all_elements
        .iter()
        .copied()
        .filter(|&n| {
            doc.is_element(n)
                && styles.get(n).map(|s| s.position) == Some(brows12_css::values::Position::Fixed)
        })
        .collect();
    for n in fixed.iter().copied() {
        let (Some(style), Some(rect)) = (styles.get(n), result.rects.get(&n).copied()) else {
            continue;
        };
        let dx = resolve_inset_x(style, rect, viewport.width) - rect.x;
        let dy = resolve_inset_y(style, rect, viewport.height) - rect.y;
        if let Some(r) = result.rects.get_mut(&n) {
            r.x += dx;
            r.y += dy;
        }
        shift_subtree(&mut result.rects, &children_of, n, dx, dy, 0);
        result.fixed_nodes.push(n);
    }

    // CSS transform translate: shift node + descendants; keep full transform
    // for the compositor/paint (rotate + scale).
    let transformed: Vec<(NodeId, brows12_css::values::Transform)> = all_elements
        .iter()
        .copied()
        .filter_map(|n| {
            let s = styles.get(n)?;
            if doc.is_element(n) && !s.transform.is_identity() {
                Some((n, s.transform))
            } else {
                None
            }
        })
        .collect();
    for (n, t) in transformed.iter().copied() {
        if t.tx != 0.0 || t.ty != 0.0 {
            shift_subtree(&mut result.rects, &children_of, n, t.tx, t.ty, 0);
        }
        result.transforms.insert(n, t);
    }

    // Content bounds: the max extent of any laid-out box (not just body).
    let mut max_h = 0.0f32;
    let mut max_w = 0.0f32;
    for r in result.rects.values() {
        max_h = max_h.max(r.y + r.height);
        max_w = max_w.max(r.x + r.width);
    }
    result.content_height = max_h.max(viewport.height);
    result.content_width = max_w.max(viewport.width);
    result
}

/// Extract a px value from a resolved `Len` (percent contributes 0 here;
/// static-position estimation is a v1 approximation).
fn len_px(v: &Len) -> f32 {
    match v {
        Len::Px(px) => *px,
        Len::Percent(_) => 0.0,
    }
}

/// X position for a fixed/absolute box inside its containing block.
fn resolve_inset_x(style: &ComputedStyle, rect: Rect, cb_width: f32) -> f32 {
    let left = inset_px(style.insets.left, cb_width);
    let right = inset_px(style.insets.right, cb_width);
    if let Some(l) = left {
        return l + offset_for_margin(style.margin.left);
    }
    if let Some(r) = right {
        return cb_width - r - rect.width + offset_for_margin(style.margin.right);
    }
    rect.x
}

fn resolve_inset_y(style: &ComputedStyle, rect: Rect, cb_height: f32) -> f32 {
    let top = inset_px(style.insets.top, cb_height);
    let bottom = inset_px(style.insets.bottom, cb_height);
    if let Some(t) = top {
        return t + offset_for_margin(style.margin.top);
    }
    if let Some(b) = bottom {
        return cb_height - b - rect.height + offset_for_margin(style.margin.bottom);
    }
    rect.y
}

fn inset_px(v: AutoPx, basis: f32) -> Option<f32> {
    match v {
        AutoPx::Auto => None,
        AutoPx::Len(Len::Px(px)) => Some(px),
        AutoPx::Len(Len::Percent(p)) => Some(p * basis),
    }
}

fn offset_for_margin(v: AutoPx) -> f32 {
    match v {
        AutoPx::Len(Len::Px(px)) => px,
        AutoPx::Len(Len::Percent(p)) => p * 16.0,
        AutoPx::Auto => 0.0,
    }
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
