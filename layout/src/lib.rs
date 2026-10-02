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

use brows12_css::values::{AutoPx, ClearSide, Display, FloatSide, Len, Position as CssPosition, TextAlign};
use brows12_css::ComputedStyle;
use brows12_html::{Document, NodeData, NodeId};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

pub mod inline;
pub use inline::{
    apply_alignment, collapse_ws, FloatBand, InlineFlowLayout, InlineItem, InlineLine, InlineSegment,
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

/// A placed float box (CSS 2.1 §9.5): removed from normal flow, shifted
/// left/right, with following line boxes shortened around it.
#[derive(Debug, Clone, Copy)]
pub struct PlacedFloat {
    pub node: NodeId,
    /// `true` for `float: left`, `false` for `float: right`.
    pub left: bool,
    /// Final absolute rectangle.
    pub rect: Rect,
    /// The containing block (parent element): floats only affect line
    /// boxes and blocks inside this subtree.
    pub cb: NodeId,
}

/// Table slot assignments for tables that contain colspan/rowspan cells
/// (CSS 2.1 §17). Such tables map onto a grid; each cell gets an explicit
/// column/row placement. Tables without spans keep the anonymous-flex
/// mapping (proven on HN/Wikipedia).
#[derive(Debug, Default)]
pub struct TableSpans {
    /// Tables mapped to grid: table node -> (max column count).
    pub tables: HashMap<NodeId, usize>,
    /// Cell placement: cell node -> (col0, colspan, row0, rowspan).
    pub cells: HashMap<NodeId, (u16, u16, u16, u16)>,
}

/// Compute table slot assignments (the CSS 2.1 table algorithm's
/// column-assignment step) for every table that has spanning cells.
fn compute_table_spans(
    doc: &Document,
    styles: &brows12_css::StyleMap,
) -> TableSpans {
    let mut out = TableSpans::default();
    let Some(body) = doc.body().or_else(|| doc.document_element()) else {
        return out;
    };
    // Walk the whole tree; each table gets its own slot pass.
    fn walk(
        doc: &Document,
        styles: &brows12_css::StyleMap,
        node: NodeId,
        out: &mut TableSpans,
    ) {
        if !doc.is_element(node) {
            for &c in &doc.node(node).children {
                walk(doc, styles, c, out);
            }
            return;
        }
        let Some(style) = styles.get(node) else {
            for &c in &doc.node(node).children {
                walk(doc, styles, c, out);
            }
            return;
        };
        if style.display == Display::Table {
            assign_table_slots(doc, styles, node, out);
            // Nested tables are handled by their own assignment (the walk
            // below re-enters cells; assign_table_slots prunes nested
            // tables, this walk covers them).
        }
        for &c in &doc.node(node).children {
            walk(doc, styles, c, out);
        }
    }
    walk(doc, styles, body, &mut out);
    out
}

/// Rows of a table: TableRow elements that directly contain cells, with
/// row-group containers (also mapped to TableRow) recursed through.
/// Nested tables are pruned (they compute their own slots).
fn table_rows_of(
    doc: &Document,
    styles: &brows12_css::StyleMap,
    table: NodeId,
) -> Vec<NodeId> {
    let mut rows = Vec::new();
    fn visit(doc: &Document, styles: &brows12_css::StyleMap, node: NodeId, rows: &mut Vec<NodeId>) {
        for &c in &doc.node(node).children {
            let Some(st) = styles.get(c) else { continue };
            if !doc.is_element(c) {
                continue;
            }
            match st.display {
                Display::TableRow => {
                    let is_group = doc.node(c).children.iter().any(|&g| {
                        doc.is_element(g) && styles.get(g).map(|s| s.display) == Some(Display::TableRow)
                    });
                    if is_group {
                        visit(doc, styles, c, rows);
                    } else {
                        rows.push(c);
                    }
                }
                Display::Table => {} // nested table: pruned
                _ => visit(doc, styles, c, rows),
            }
        }
    }
    visit(doc, styles, table, &mut rows);
    rows
}

fn attr_num(doc: &Document, node: NodeId, name: &str, lo: u16, hi: u16) -> u16 {
    doc.attr(node, name)
        .and_then(|v| v.trim().parse::<u16>().ok())
        .map(|v| v.clamp(lo, hi))
        .unwrap_or(1)
}

/// The slot-assignment pass for one table (CSS 2.1 §17.2.1 fixed + auto
/// layout share this step): cells take the next free column, honouring
/// rowspan occupancy from earlier rows.
fn assign_table_slots(
    doc: &Document,
    styles: &brows12_css::StyleMap,
    table: NodeId,
    out: &mut TableSpans,
) {
    let rows = table_rows_of(doc, styles, table);
    // Only map to grid when some cell actually spans.
    let raw_attr = |n: NodeId, name: &str| -> u16 {
        doc.attr(n, name).and_then(|v| v.trim().parse::<u16>().ok()).unwrap_or(1)
    };
    let has_span = rows.iter().any(|&r| {
        doc.node(r).children.iter().any(|&c| {
            doc.is_element(c)
                && (raw_attr(c, "colspan") > 1 || raw_attr(c, "rowspan") > 1)
        })
    });
    if !has_span {
        return;
    }
    let mut occupied: std::collections::HashSet<(usize, usize)> = Default::default();
    let mut max_cols = 0usize;
    for (r, &row) in rows.iter().enumerate() {
        let mut cursor = 0usize;
        for &c in &doc.node(row).children {
            let Some(st) = styles.get(c) else { continue };
            if !doc.is_element(c) || st.display != Display::TableCell {
                continue;
            }
            let colspan = attr_num(doc, c, "colspan", 1, 128);
            let rowspan = attr_num(doc, c, "rowspan", 1, 64);
            while occupied.contains(&(r, cursor)) {
                cursor += 1;
            }
            out.cells.insert(c, (cursor as u16, colspan, r as u16, rowspan));
            for i in 0..rowspan as usize {
                for j in 0..colspan as usize {
                    occupied.insert((r + i, cursor + j));
                }
            }
            cursor += colspan as usize;
            max_cols = max_cols.max(cursor);
        }
    }
    if max_cols > 0 {
        out.tables.insert(table, max_cols);
    }
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
    /// Placed floats in document order (for line banding and paint).
    pub floats: Vec<PlacedFloat>,
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

/// `min-width/height`: auto = taffy's content-based auto minimum.
fn taffy_auto_min(v: AutoPx) -> taffy::LengthPercentageAuto {
    match v {
        AutoPx::Auto => taffy::LengthPercentageAuto::auto(),
        AutoPx::Len(Len::Px(px)) => taffy::LengthPercentageAuto::length(px),
        AutoPx::Len(Len::Percent(p)) => taffy::LengthPercentageAuto::percent(p),
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
        CssPosition::Static => {
            // Floats are removed from normal flow: lay them out as taffy
            // absolutes so sibling boxes ignore them; the float post-pass
            // then positions them with the CSS 2.1 float rules.
            if style.float != FloatSide::None {
                taffy::Position::Absolute
            } else {
                taffy::Position::Relative // taffy default is Relative
            }
        }
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

fn build_taffy_style(
    style: &ComputedStyle,
    spans: &TableSpans,
    node: NodeId,
    parent_display: Option<Display>,
) -> taffy::Style {
    let display = match style.display {
        Display::None => taffy::Display::None,
        Display::Flex | Display::Table | Display::TableRow => taffy::Display::Flex,
        Display::Block | Display::Inline | Display::TableCell => taffy::Display::Block,
        Display::Grid => taffy::Display::Grid,
    };
    let mut taffy_style = taffy::Style {
        display,
        position: taffy_position(style),
        inset: taffy_inset(style),
        size: taffy::Size {
            width: taffy_dimension(style.width),
            height: taffy_dimension(style.height),
        },
        max_size: {
            // max-height is not modelled yet: keep the vertical axis unconstrained.
            let _ = &style;
            taffy::Size {
                width: taffy_auto_min(style.max_width),
                height: taffy::LengthPercentageAuto::auto(),
            }
        },
        min_size: {
            // CSS: the automatic minimum size (min-width:auto flooring at
            // content) applies to flex/grid ITEMS only. Block children have
            // min-width 0 and simply overflow their parent. taffy applies
            // the content floor to block children too, which blew up the
            // Vector-2022 containers to max-content widths, so floor at 0
            // outside flex/grid parents (an explicit min-width still wins).
            let item = matches!(parent_display, Some(Display::Flex) | Some(Display::Grid));
            let conv = |v: &AutoPx| -> taffy::LengthPercentageAuto {
                if !item && matches!(v, AutoPx::Auto) {
                    return taffy::LengthPercentageAuto::length(0.0);
                }
                taffy_auto_min(*v)
            };
            taffy::Size { width: conv(&style.min_width), height: conv(&style.min_height) }
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

    // CSS tables map onto anonymous flex structures: table = column flex,
    // row = row flex with stretching items, cell = block flex item.
    if style.display == Display::Table {
        taffy_style.flex_direction = taffy::FlexDirection::Column;
        taffy_style.align_items = Some(taffy::AlignItems::STRETCH);
    }
    if style.display == Display::TableRow {
        taffy_style.flex_direction = taffy::FlexDirection::Row;
        taffy_style.align_items = Some(taffy::AlignItems::STRETCH);
        taffy_style.flex_grow = 1.0;
    }
    if style.display == Display::TableCell {
        // Cells size to content and shrink when the row overflows;
        // leftover row space stays trailing (close to Chrome's column
        // sizing for content-driven tables).
        taffy_style.flex_grow = 0.0;
        taffy_style.flex_shrink = 1.0;
    }
    // Shrink-to-fit boxes (floats, absolutes) size to their max-content in
    // taffy, which can exceed the containing block. Chrome caps fit-content
    // at the available space (CSS2 §10.3.7): apply the same cap via
    // max-width: 100% so wide content wraps instead of exploding the box.
    if (style.float != FloatSide::None
        || style.position == CssPosition::Absolute
        || style.position == CssPosition::Fixed)
        && style.width == AutoPx::Auto
        && style.max_width == AutoPx::Auto
    {
        taffy_style.max_size.width = taffy::LengthPercentageAuto::percent(1.0);
    }

    // ---- CSS grid: templates, auto tracks, flow, item placement ----
    if style.display == Display::Grid {
        let tf = |t: &brows12_css::values::GridTrackSize| -> taffy::style::TrackSizingFunction {
            use taffy::style::{MaxTrackSizingFunction as Max, MinTrackSizingFunction as Min};
            use taffy::prelude::TaffyAuto;
            fn min_side(g: &brows12_css::values::GridTrackSize) -> Min {
                match g {
                    brows12_css::values::GridTrackSize::Auto => Min::auto(),
                    brows12_css::values::GridTrackSize::Px(px) => Min::length(*px),
                    brows12_css::values::GridTrackSize::Percent(p) => Min::percent(*p),
                    brows12_css::values::GridTrackSize::MinContent => Min::min_content(),
                    brows12_css::values::GridTrackSize::MaxContent => Min::max_content(),
                    // `fr` is a max-function only; the min side stays auto.
                    brows12_css::values::GridTrackSize::Fr(_) => Min::auto(),
                    // Nested minmax on the min side: use its inner min.
                    brows12_css::values::GridTrackSize::MinMax { min, .. } => min_side(min),
                }
            }
            let side = min_side;
            match t {
                brows12_css::values::GridTrackSize::MinMax { min, max } => {
                    let lo = side(min);
                    let hi = match &**max {
                        brows12_css::values::GridTrackSize::Fr(f) => Max::fr(*f),
                        brows12_css::values::GridTrackSize::Auto => Max::auto(),
                        brows12_css::values::GridTrackSize::Px(px) => Max::length(*px),
                        brows12_css::values::GridTrackSize::Percent(p) => Max::percent(*p),
                        brows12_css::values::GridTrackSize::MinContent => Max::min_content(),
                        brows12_css::values::GridTrackSize::MaxContent => Max::max_content(),
                        brows12_css::values::GridTrackSize::MinMax { .. } => Max::auto(),
                    };
                    taffy::style::TrackSizingFunction { min: lo, max: hi }
                }
                brows12_css::values::GridTrackSize::Fr(f) => {
                    taffy::style::TrackSizingFunction { min: Min::auto(), max: Max::fr(*f) }
                }
                other => {
                    let lo = side(other);
                    let hi = match other {
                        brows12_css::values::GridTrackSize::Px(px) => Max::length(*px),
                        brows12_css::values::GridTrackSize::Percent(p) => Max::percent(*p),
                        brows12_css::values::GridTrackSize::MinContent => Max::min_content(),
                        brows12_css::values::GridTrackSize::MaxContent => Max::max_content(),
                        _ => Max::auto(),
                    };
                    taffy::style::TrackSizingFunction { min: lo, max: hi }
                }
            }
        };
        let single = |t: &brows12_css::values::GridTrackSize| {
            taffy::style::GridTemplateComponent::Single(tf(t))
        };
        if !style.grid_template_columns.is_empty() {
            taffy_style.grid_template_columns =
                style.grid_template_columns.iter().map(single).collect();
        }
        if !style.grid_template_rows.is_empty() {
            taffy_style.grid_template_rows = style.grid_template_rows.iter().map(single).collect();
        }
        // Auto tracks are plain sizing functions (not template components).
        use taffy::prelude::TaffyAuto;
        if !style.grid_auto_columns.is_empty() {
            taffy_style.grid_auto_columns = style.grid_auto_columns.iter().map(tf).collect();
        } else {
            taffy_style.grid_auto_columns = vec![taffy::style::TrackSizingFunction::AUTO];
        }
        if !style.grid_auto_rows.is_empty() {
            taffy_style.grid_auto_rows = style.grid_auto_rows.iter().map(tf).collect();
        } else {
            taffy_style.grid_auto_rows = vec![taffy::style::TrackSizingFunction::AUTO];
        }
        taffy_style.grid_auto_flow = match (style.grid_auto_flow_column, style.grid_auto_flow_dense) {
            (false, false) => taffy::style::GridAutoFlow::Row,
            (true, false) => taffy::style::GridAutoFlow::Column,
            (false, true) => taffy::style::GridAutoFlow::RowDense,
            (true, true) => taffy::style::GridAutoFlow::ColumnDense,
        };
    }
    // Item placement applies to any node (only meaningful under a grid
    // parent; harmless elsewhere).
    {
        let place = |spec: brows12_css::values::GridLineSpec| -> taffy::style::GridPlacement {
            match spec {
                brows12_css::values::GridLineSpec::Auto => taffy::style::GridPlacement::Auto,
                brows12_css::values::GridLineSpec::Line(i) => {
                    use taffy::prelude::TaffyGridLine;
                    taffy::style::GridPlacement::from_line_index(i)
                }
                brows12_css::values::GridLineSpec::Span(n) => {
                    taffy::style::GridPlacement::Span(n.max(1))
                }
            }
        };
        taffy_style.grid_column = taffy::Line {
            start: place(style.grid_column.0),
            end: place(style.grid_column.1),
        };
        taffy_style.grid_row = taffy::Line {
            start: place(style.grid_row.0),
            end: place(style.grid_row.1),
        };
    }
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

    // Spanning table cells: explicit grid placement from the slot
    // algorithm (grid-mapped tables only).
    if style.display == Display::TableCell {
        if let Some(&(col, colspan, row, rowspan)) = spans.cells.get(&node) {
            use taffy::prelude::TaffyGridLine;
            taffy_style.grid_column = taffy::Line {
                start: taffy::style::GridPlacement::from_line_index(col as i16 + 1),
                end: if colspan > 1 {
                    taffy::style::GridPlacement::Span(colspan)
                } else {
                    taffy::style::GridPlacement::Auto
                },
            };
            taffy_style.grid_row = taffy::Line {
                start: taffy::style::GridPlacement::from_line_index(row as i16 + 1),
                end: if rowspan > 1 {
                    taffy::style::GridPlacement::Span(rowspan)
                } else {
                    taffy::style::GridPlacement::Auto
                },
            };
        }
    }
    taffy_style
}

/// Apply CSS `text-transform` to a text chunk.
fn transform_text(t: &str, tt: brows12_css::values::TextTransform) -> String {
    match tt {
        brows12_css::values::TextTransform::None => t.to_string(),
        brows12_css::values::TextTransform::Uppercase => t.to_uppercase(),
        brows12_css::values::TextTransform::Lowercase => t.to_lowercase(),
        brows12_css::values::TextTransform::Capitalize => {
            let mut out = String::with_capacity(t.len());
            let mut at_word_start = true;
            for ch in t.chars() {
                if ch.is_whitespace() {
                    at_word_start = true;
                    out.push(ch);
                } else if at_word_start {
                    out.extend(ch.to_uppercase());
                    at_word_start = false;
                } else {
                    out.push(ch);
                }
            }
            out
        }
    }
}

fn leaf_context(style: &ComputedStyle, text: &str) -> LeafContext {
    LeafContext::Text {
        text: transform_text(text, style.text_transform),
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

    #[allow(clippy::too_many_arguments)] // state-threading over recursion
    fn build(
        doc: &Document,
        styles: &brows12_css::StyleMap,
        tree: &mut taffy::TaffyTree<LeafContext>,
        node_ids: &mut HashMap<NodeId, taffy::NodeId>,
        image_sizes: &HashMap<NodeId, (u32, u32)>,
        inline_ctx: &mut HashMap<NodeId, (Arc<Vec<InlineItem>>, TextAlign)>,
        covered: &mut HashSet<NodeId>,
        spans: &TableSpans,
        shrink_ctx: &HashSet<NodeId>,
        parent_display: Option<Display>,
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
                let tnode = tree
                    .new_leaf_with_context(build_taffy_style(style, spans, node, parent_display), ctx)
                    .ok()?;
                node_ids.insert(node, tnode);
                Some(tnode)
            }
            NodeData::Element { .. } => {
                // Leaf elements with intrinsic size (img).
                if doc.node(node).children.is_empty() {
                    if let Some(&(w, h)) = image_sizes.get(&node) {
                        let taffy_style = build_taffy_style(style, spans, node, parent_display);
                        let ctx = LeafContext::Image { intrinsic_width: w, intrinsic_height: h };
                        let tnode = tree.new_leaf_with_context(taffy_style, ctx).ok()?;
                        node_ids.insert(node, tnode);
                        return Some(tnode);
                    }
                }

                // Tables with spanning cells map onto a grid: cells become
                // direct grid items with explicit slot placement (row boxes
                // are skipped); the slot algorithm ran in compute_table_spans.
                if style.display == Display::Table {
                    // Grid-map spanning tables, EXCEPT width-less tables
                    // inside shrink-to-fit contexts (floats/absolutes): an
                    // auto-width grid there sizes to a huge max-content and
                    // explodes the float. Width-less shrink-context tables
                    // keep the flex mapping (content-sized columns).
                    if !shrink_ctx.contains(&node) || style.width != AutoPx::Auto {
                        if let Some(&max_cols) = spans.tables.get(&node) {
                            use taffy::prelude::TaffyAuto;
                        let mut taffy_style = build_taffy_style(style, spans, node, parent_display);
                        taffy_style.display = taffy::Display::Grid;
                        taffy_style.grid_template_columns = (0..max_cols)
                            .map(|_| {
                                taffy::style::GridTemplateComponent::Single(
                                    taffy::style::TrackSizingFunction::AUTO,
                                )
                            })
                            .collect();
                        taffy_style.grid_auto_rows = vec![taffy::style::TrackSizingFunction::AUTO];
                        let mut children: Vec<taffy::NodeId> = Vec::new();
                        for row in table_rows_of(doc, styles, node) {
                            for &cell in &doc.node(row).children {
                                let Some(cs) = styles.get(cell) else { continue };
                                if !doc.is_element(cell) || cs.display != Display::TableCell {
                                    continue;
                                }
                                if let Some(t) = build(
                                    doc,
                                    styles,
                                    tree,
                                    node_ids,
                                    image_sizes,
                                    inline_ctx,
                                    covered,
                                    spans,
                                    shrink_ctx,
                                    Some(style.display),
                                    cell,
                                ) {
                                    children.push(t);
                                }
                            }
                        }
                        let tnode = if children.is_empty() {
                            tree.new_leaf(taffy_style).ok()?
                        } else {
                            tree.new_with_children(taffy_style, &children).ok()?
                        };
                        node_ids.insert(node, tnode);
                        return Some(tnode);
                        }
                    }
                }

                let taffy_style = build_taffy_style(style, spans, node, parent_display);
                // Partition children into inline runs and block children
                // (CSS anonymous block boxes).
                let pieces = group_children(doc, styles, image_sizes, node);
                let mut children: Vec<taffy::NodeId> = Vec::new();
                for piece in pieces {
                    match piece {
                        Piece::Block(c) => {
                            if let Some(t) = build(
                                doc,
                                styles,
                                tree,
                                node_ids,
                                image_sizes,
                                inline_ctx,
                                covered,
                                spans,
                                shrink_ctx,
                                Some(style.display),
                                c,
                            ) {
                                children.push(t);
                            }
                        }
                        Piece::Inline(members) => {
                            if let Some(t) = build_inline_group(
                                doc,
                                styles,
                                tree,
                                node_ids,
                                image_sizes,
                                inline_ctx,
                                covered,
                                node,
                                &members,
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
                    if let Some(t) = build(
                        doc,
                        styles,
                        tree,
                        node_ids,
                        image_sizes,
                        inline_ctx,
                        covered,
                        spans,
                        shrink_ctx,
                        parent_display,
                        c,
                    ) {
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
        if matches!(
            styles.get(node).map(|s| s.display),
            Some(Display::Flex) | Some(Display::Grid)
        ) {
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
                let text = transform_text(&text, style.text_transform);
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
                    items.push(InlineItem::newline(style.font_size, style.line_height_px(), style));
                    return;
                }
                let bg2 =
                    if style.background_color[3] > 0 { Some(style.background_color) } else { bg };
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
    let table_spans = compute_table_spans(doc, styles);
    // Nodes inside float/absolute/fixed subtrees (shrink-to-fit contexts).
    let mut shrink_ctx: HashSet<NodeId> = HashSet::new();
    fn collect_shrink(
        doc: &Document,
        styles: &brows12_css::StyleMap,
        node: NodeId,
        inside: bool,
        out: &mut HashSet<NodeId>,
    ) {
        let inside = inside
            || styles
                .get(node)
                .map(|s| {
                    s.float != FloatSide::None
                        || s.position == CssPosition::Absolute
                        || s.position == CssPosition::Fixed
                })
                .unwrap_or(false);
        if inside {
            out.insert(node);
        }
        for &c in &doc.node(node).children {
            collect_shrink(doc, styles, c, inside, out);
        }
    }
    collect_shrink(doc, styles, start, false, &mut shrink_ctx);
    let Some(root_taffy) = build(
        doc,
        styles,
        &mut tree,
        &mut node_ids,
        image_sizes,
        &mut inline_ctx,
        &mut covered,
        &table_spans,
        &shrink_ctx,
        None,
        start,
    ) else {
        return LayoutResult::default();
    };

    // Collapse a single html>body chain so body fills the viewport root.
    let layout_root = root_taffy;
    let measurer_ref = &measurer;
    // Shared band store for the two-pass compute: pass 1 measures inline
    // groups at full width, the float pass derives per-group bands, pass 2
    // re-measures with bands so wrapping heights flow to later siblings.
    let bands_cell: RefCell<HashMap<taffy::NodeId, Vec<FloatBand>>> = RefCell::new(HashMap::new());
    let measure_fn = |input: taffy::LayoutInput,
                      node: taffy::NodeId,
                      ctx: Option<&mut LeafContext>,
                      _style: &taffy::Style|
     -> taffy::LayoutOutput {
        if let Some(leaf) = ctx {
            // Intrinsic sizing requests must be honoured exactly: MaxContent
            // measures unwrapped; MinContent wraps at every opportunity
            // (widest token). Returning max-content for MinContent requests
            // made `1fr` (= minmax(auto, 1fr)) grid/flex tracks explode to
            // the full unwrapped line width (10625px on Wikipedia).
            let max_width = input.known_dimensions.width.or(match input.available_space.width {
                taffy::AvailableSpace::Definite(w) => Some(w),
                taffy::AvailableSpace::MaxContent => None,
                taffy::AvailableSpace::MinContent => Some(0.0),
            });
            let (w, h) = match leaf {
                LeafContext::Inline { items, .. } => {
                    let bands = bands_cell
                        .borrow()
                        .get(&node)
                        .cloned()
                        .unwrap_or_default();
                    let flow = inline::layout_inline_banded(
                        measurer_ref,
                        items,
                        max_width,
                        &bands,
                    );
                    (flow.width, flow.height.max(0.0))
                }
                other => measurer_ref.measure(other, max_width),
            };
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

    // Root max-width (e.g. example.com's body { max-width: 26em }): taffy
    // sizes the root to the given available space and ignores the root's
    // own max-size, so constrain the available width ourselves and center
    // the root box afterwards (margin:auto behaviour).
    let root_style = styles.get(start);
    let avail_width = root_style
        .and_then(|s| inset_px(s.max_width, viewport.width))
        .map(|mw| mw.min(viewport.width).max(1.0))
        .unwrap_or(viewport.width);
    let run_compute = |tree: &mut taffy::TaffyTree<LeafContext>,
                       root: taffy::NodeId,
                       measure: &dyn Fn(
        taffy::LayoutInput,
        taffy::NodeId,
        Option<&mut LeafContext>,
        &taffy::Style,
    ) -> taffy::LayoutOutput|
     -> Result<(), taffy::TaffyError> {
        tree.compute_layout_with_measure(
            root,
            taffy::Size {
                width: taffy::AvailableSpace::Definite(avail_width),
                height: taffy::AvailableSpace::Definite(viewport.height),
            },
            measure,
        )
    };
    let _ = run_compute(&mut tree, layout_root, &measure_fn);

    // Extract absolute rects via depth-first accumulation.
    let taffy_to_dom: HashMap<taffy::NodeId, NodeId> =
        node_ids.iter().map(|(d, t)| (*t, *d)).collect();
    #[allow(clippy::too_many_arguments)]
    fn extract(
        doc: &Document,
        styles: &brows12_css::StyleMap,
        parent_of: &HashMap<NodeId, NodeId>,
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
                if layout.size.width > 4000.0 && std::env::var("BROWS_TRACE").is_ok() {
                    let mut cur = Some(*dom);
                    let mut chain = Vec::new();
                    while let Some(c) = cur {
                        let desc = match &doc.node(c).data {
                            NodeData::Element { name, .. } => {
                                let cls = doc.attr(c, "class").unwrap_or("");
                                let st = styles.get(c);
                                format!(
                                    "<{name} .{cls}> display={:?} pos={:?} float={:?} ws={:?} w={:?}",
                                    st.map(|s| s.display),
                                    st.map(|s| s.position),
                                    st.map(|s| s.float),
                                    st.map(|s| s.white_space),
                                    st.map(|s| s.width)
                                )
                            }
                            _ => "?".into(),
                        };
                        chain.push(desc);
                        cur = parent_of.get(&c).copied();
                    }
                    eprintln!("WIDE-CHAIN: {chain:?}");
                }
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
                extract(doc, styles, parent_of, taffy_to_dom, tree, rects, child, (x, y));
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

    // ---- Tree maps shared by all post-passes (pre-order doc order) ----
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
    let mut parent_of: HashMap<NodeId, NodeId> = HashMap::new();
    fn collect_parents(doc: &Document, node: NodeId, parent_of: &mut HashMap<NodeId, NodeId>) {
        for &c in &doc.node(node).children {
            parent_of.insert(c, node);
            collect_parents(doc, c, parent_of);
        }
    }
    collect_parents(doc, start, &mut parent_of);
    let doc_order: HashMap<NodeId, usize> =
        all_elements.iter().enumerate().map(|(i, &n)| (n, i)).collect();

    // ---- Pass 1 rects (no bands) → place floats → derive bands ----
    let mut rects1: HashMap<NodeId, Rect> = HashMap::new();
    extract(doc, styles, &parent_of, &taffy_to_dom, &tree, &mut rects1, root_taffy, (0.0, 0.0));
    let floats1 = place_floats(doc, styles, &mut rects1, &children_of, &parent_of, &all_elements);
    {
        let mut bmap = bands_cell.borrow_mut();
        for dom_id in inline_ctx.keys() {
            let (Some(gr), Some(&tn)) = (rects1.get(dom_id), node_ids.get(dom_id)) else {
                continue;
            };
            let leaf_w = tree.layout(tn).map(|l| l.size.width).unwrap_or(gr.width);
            let w = container_content_width(styles, &parent_of, &rects1, *dom_id, leaf_w);
            let bands = bands_for_group(&parent_of, *dom_id, *gr, w, &floats1);
            if !bands.is_empty() {
                bmap.insert(tn, bands);
            }
        }
    }

    // ---- Pass 2: recompute with bands (wrapping heights reach siblings) ----
    // taffy caches leaf measurements; identical inputs would return the
    // pass-1 heights. Invalidate every node so measure runs again with the
    // new bands.
    for &tn in node_ids.values() {
        let _ = tree.mark_dirty(tn);
    }
    let _ = run_compute(&mut tree, layout_root, &measure_fn);

    // ---- Final extraction + float placement on final rects ----
    let mut result = LayoutResult::default();
    extract(doc, styles, &parent_of, &taffy_to_dom, &tree, &mut result.rects, root_taffy, (0.0, 0.0));

    // Center a width-constrained root with auto horizontal margins.
    if avail_width < viewport.width {
        let auto_margins = root_style.map(|s| {
            matches!(s.margin.left, AutoPx::Auto) && matches!(s.margin.right, AutoPx::Auto)
        }) == Some(true);
        let auto_single = root_style
            .map(|s| matches!(s.margin.left, AutoPx::Auto) || matches!(s.margin.right, AutoPx::Auto))
            == Some(true);
        if auto_margins || auto_single {
            let dx = ((viewport.width - avail_width) / 2.0).max(0.0);
            if dx > 0.0 {
                for r in result.rects.values_mut() {
                    r.x += dx;
                }
            }
        }
    }
    result.floats =
        place_floats(doc, styles, &mut result.rects, &children_of, &parent_of, &all_elements);

    // ---- clear + BFC float-avoidance (pushes blocks below floats) ----
    apply_clear_and_bfc(
        doc,
        styles,
        &mut result.rects,
        &children_of,
        &parent_of,
        &doc_order,
        &result.floats,
    );

    // ---- Inline flows: final shape at each group's resolved box width ----
    for (dom_id, (items, align)) in inline_ctx.iter() {
        let Some(&tn) = node_ids.get(dom_id) else { continue };
        let w = match tree.layout(tn) {
            Ok(l) => l.size.width,
            Err(_) => continue,
        };
        let (Some(gr), Some(first)) = (result.rects.get(dom_id).copied(), Some(*dom_id)) else {
            continue;
        };
        let _ = first;
        // Lines span the containing block's content width (the leaf itself
        // shrink-wraps), shortened by float bands.
        let gw = container_content_width(styles, &parent_of, &result.rects, *dom_id, w);
        let bands = bands_for_group(&parent_of, *dom_id, gr, gw, &result.floats);
        let mut flow = inline::layout_inline_banded(measurer, items, Some(gw), &bands);
        apply_alignment(&mut flow, gw, *align);
        result.inline_flows.insert(*dom_id, flow);
    }
    result.inline_covered = covered;
    if std::env::var("BROWS_DEBUG").is_ok() {
        eprintln!(
            "FLOWS {} groups, {} flows, {} covered, {} rects",
            inline_ctx.len(),
            result.inline_flows.len(),
            result.inline_covered.len(),
            result.rects.len()
        );
    }

    // ---- v0.2 post-pass: fixed anchoring + transforms + content bounds ----
    // (children_of / parent_of / shift_subtree are defined above, shared
    // with the float passes)

    // position: absolute — resolve insets ourselves against the parent's
    // padding box (taffy does not resolve percentage top/bottom insets on
    // absolute children). Auto top+bottom keep the STATIC flow position
    // (CSS 2.1 §10.3.7); same for auto left+right on the x axis.
    {
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
                // Single application: the subtree shift alone moves the box
                // and its descendants to the resolved position (assigning
                // nx on top of the shift would double-apply the delta).
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

/// Shift every descendant of `node` by `dx`/`dy` (recursive, pre-order).
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

/// Is `candidate` an ancestor-or-self of `node`?
fn in_chain(parent_of: &HashMap<NodeId, NodeId>, node: NodeId, candidate: NodeId) -> bool {
    let mut cur = Some(node);
    while let Some(c) = cur {
        if c == candidate {
            return true;
        }
        cur = parent_of.get(&c).copied();
    }
    false
}

/// Content width of the block container that owns an inline group. The
/// anonymous inline leaf shrink-wraps to its content, so float bands and
/// line limits must be resolved against the CONTAINING block's content
/// width (CSS 2.1 §9.5: line boxes span the containing block, shortened
/// by floats) — otherwise short paragraphs beside floats never wrap.
fn container_content_width(
    styles: &brows12_css::StyleMap,
    parent_of: &HashMap<NodeId, NodeId>,
    rects: &HashMap<NodeId, Rect>,
    dom_id: NodeId,
    fallback: f32,
) -> f32 {
    let Some(&parent) = parent_of.get(&dom_id) else { return fallback };
    let Some(pr) = rects.get(&parent) else { return fallback };
    let mut w = pr.width;
    if let Some(ps) = styles.get(parent) {
        w -= len_px(&ps.padding.left)
            + len_px(&ps.padding.right)
            + len_px(&ps.border_width.left)
            + len_px(&ps.border_width.right);
    }
    if w > 0.5 { w } else { fallback }
}

/// Place every floated element with the CSS 2.1 §9.5 float rules:
/// left/right edge stacking against earlier floats of the same containing
/// block, collision push-down, `clear`, and the "not above earlier float
/// tops" constraint. Floats leave normal flow, so only their own rect and
/// subtree move; the nearest block-formatting-context ancestor grows to
/// contain them.
fn place_floats(
    doc: &Document,
    styles: &brows12_css::StyleMap,
    rects: &mut HashMap<NodeId, Rect>,
    children_of: &HashMap<NodeId, Vec<NodeId>>,
    parent_of: &HashMap<NodeId, NodeId>,
    doc_order: &[NodeId],
) -> Vec<PlacedFloat> {
    let mut placed: Vec<PlacedFloat> = Vec::new();
    let mut top_floor: HashMap<NodeId, f32> = HashMap::new();

    for &n in doc_order {
        let Some(style) = styles.get(n) else { continue };
        if !doc.is_element(n) || style.float == FloatSide::None {
            continue;
        }
        if std::env::var("BROWS_DEBUG").is_ok() {
            let name = match &doc.node(n).data {
                NodeData::Element { name, .. } => name.clone(),
                _ => "?".into(),
            };
            eprintln!("FLOAT n={n:?} <{name}> side={:?} rect={:?}", style.float, rects.get(&n));
        }
        let Some(&parent) = parent_of.get(&n) else { continue };
        let Some(r0) = rects.get(&n).copied() else { continue };
        if r0.width <= 0.0 || r0.height <= 0.0 {
            continue;
        }
        let Some(p_rect) = rects.get(&parent).copied() else { continue };
        let p_style = styles.get(parent);

        // Containing block: parent's padding box.
        let (cb_x, cb_y, cb_w) = match p_style {
            Some(ps) => (
                p_rect.x + len_px(&ps.border_width.left),
                p_rect.y + len_px(&ps.border_width.top),
                (p_rect.width
                    - len_px(&ps.border_width.left)
                    - len_px(&ps.border_width.right))
                .max(0.0),
            ),
            None => (p_rect.x, p_rect.y, p_rect.width),
        };
        let cb_right = cb_x + cb_w;

        // Static position (where the box would have been in flow): after
        // the previous in-flow siblings.
        let mut y = cb_y + p_style.map(|ps| len_px(&ps.padding.top)).unwrap_or(0.0);
        if let Some(sibs) = children_of.get(&parent) {
            for &sib in sibs {
                if sib == n {
                    break;
                }
                let Some(ss) = styles.get(sib) else { continue };
                if ss.display == Display::None
                    || ss.float != FloatSide::None
                    || ss.position == CssPosition::Absolute
                    || ss.position == CssPosition::Fixed
                {
                    continue;
                }
                if let Some(sr) = rects.get(&sib) {
                    let mb = match ss.margin.bottom {
                        AutoPx::Len(Len::Px(px)) => px,
                        _ => 0.0,
                    };
                    y = y.max(sr.y + sr.height + mb);
                }
            }
        }
        // Rule 5: not above the top of any earlier float in this CB.
        if let Some(&tf) = top_floor.get(&parent) {
            y = y.max(tf);
        }
        // `clear` on the float itself.
        for pf in &placed {
            if !in_chain(parent_of, n, pf.cb) {
                continue;
            }
            let side_match = match style.clear {
                ClearSide::Left => pf.left,
                ClearSide::Right => !pf.left,
                ClearSide::Both => true,
                ClearSide::None => false,
            };
            if side_match {
                y = y.max(pf.rect.y + pf.rect.height);
            }
        }

        // Horizontal placement with collision push-down (§9.5.1 rules 2-4).
        let w = r0.width;
        let h = r0.height;
        let mut x;
        let mut guard = 0;
        loop {
            guard += 1;
            let mut left_inner = cb_x;
            let mut right_inner = cb_right;
            let mut lowest = 0.0f32;
            let mut any_overlap = false;
            for pf in placed.iter().filter(|pf| pf.cb == parent) {
                let fr = pf.rect;
                if fr.y < y + h - 0.5 && fr.y + fr.height > y + 0.5 {
                    any_overlap = true;
                    lowest = lowest.max(fr.y + fr.height);
                    if pf.left {
                        left_inner = left_inner.max(fr.x + fr.width);
                    } else {
                        right_inner = right_inner.min(fr.x);
                    }
                }
            }
            x = if style.float == FloatSide::Left { left_inner } else { right_inner - w };
            let collide = if style.float == FloatSide::Left {
                x + w > right_inner + 0.5
            } else {
                x < left_inner - 0.5
            };
            if !collide || !any_overlap || guard > 48 || lowest <= y + 0.5 {
                break;
            }
            y = lowest;
        }

        top_floor.insert(parent, top_floor.get(&parent).copied().unwrap_or(y).max(y));

        if std::env::var("BROWS_DEBUG").is_ok() {
            eprintln!(
                "PLACE n={n:?} side={:?} cb=({cb_x:.0},{cb_y:.0} w={cb_w:.0}) r0=({:.0},{:.0} {}x{}) -> ({x:.0},{y:.0})",
                style.float, r0.x, r0.y, r0.width, r0.height
            );
        }

        let dx = x - r0.x;
        let dy = y - r0.y;
        if dx != 0.0 || dy != 0.0 {
            // Single application: the subtree shift moves the float box and
            // all descendants (never assign the absolute x on top of it —
            // that would double-apply the delta).
            shift_subtree(rects, children_of, n, dx, dy, 0);
        }
        placed.push(PlacedFloat {
            node: n,
            left: style.float == FloatSide::Left,
            rect: Rect { x, y, width: w, height: h },
            cb: parent,
        });
    }

    // The nearest block-formatting-context ancestor contains its floats:
    // grow its height to the float bottom (root, overflow != visible,
    // flex/table boxes).
    for pf in &placed {
        let mut cur = parent_of.get(&pf.node).copied();
        while let Some(a) = cur {
            let is_root = !parent_of.contains_key(&a);
            let bfc = is_root
                || styles.get(a).map(|s| {
                    s.overflow != brows12_css::values::OverflowKeyword::Visible
                        || matches!(
                            s.display,
                            Display::Flex | Display::Table | Display::TableCell
                        )
                }) == Some(true);
            if bfc {
                if let Some(ar) = rects.get_mut(&a) {
                    let bottom = pf.rect.y + pf.rect.height;
                    if bottom > ar.y + ar.height {
                        ar.height = bottom - ar.y;
                    }
                }
                break;
            }
            cur = parent_of.get(&a).copied();
        }
    }

    placed
}

/// Float bands for one inline group, in the group's LOCAL coordinates.
/// A float governs the group when the float's containing block is an
/// ancestor-or-self of the group's first member (CSS: floats shorten line
/// boxes of content in their containing block subtree).
fn bands_for_group(
    parent_of: &HashMap<NodeId, NodeId>,
    dom_id: NodeId,
    group_rect: Rect,
    group_width: f32,
    floats: &[PlacedFloat],
) -> Vec<FloatBand> {
    let mut bands = Vec::new();
    for pf in floats {
        // A float never shortens the line boxes of its own subtree.
        if in_chain(parent_of, dom_id, pf.node) {
            continue;
        }
        if !in_chain(parent_of, dom_id, pf.cb) {
            continue;
        }
        let y0 = pf.rect.y - group_rect.y;
        let y1 = y0 + pf.rect.height;
        if y1 <= 0.5 {
            continue; // float entirely above this group
        }
        if pf.left {
            let li = (pf.rect.x + pf.rect.width - group_rect.x).clamp(0.0, group_width);
            if li > 0.5 {
                bands.push(FloatBand { y0, y1, left: li, right: 0.0 });
            }
        } else {
            let ri = (group_rect.x + group_width - pf.rect.x).clamp(0.0, group_width);
            if ri > 0.5 {
                bands.push(FloatBand { y0, y1, left: 0.0, right: ri });
            }
        }
    }
    bands
}

/// `clear` (§9.5.2) and BFC float-avoidance: push blocks below matching
/// floats, shifting later siblings and growing ancestor heights so
/// nothing stacks on top of the float region.
fn apply_clear_and_bfc(
    doc: &Document,
    styles: &brows12_css::StyleMap,
    rects: &mut HashMap<NodeId, Rect>,
    children_of: &HashMap<NodeId, Vec<NodeId>>,
    parent_of: &HashMap<NodeId, NodeId>,
    doc_order: &HashMap<NodeId, usize>,
    floats: &[PlacedFloat],
) {
    for &n in doc_order.keys() {
        let Some(style) = styles.get(n) else { continue };
        if !doc.is_element(n)
            || style.display == Display::None
            || style.float != FloatSide::None
            || style.position == CssPosition::Absolute
            || style.position == CssPosition::Fixed
        {
            continue;
        }
        let wants_clear = style.clear != ClearSide::None;
        let is_bfc = matches!(
            style.overflow,
            brows12_css::values::OverflowKeyword::Hidden
                | brows12_css::values::OverflowKeyword::Scroll
                | brows12_css::values::OverflowKeyword::Auto
        );
        if !wants_clear && !is_bfc {
            continue;
        }
        let Some(r) = rects.get(&n).copied() else { continue };
        if r.width <= 0.0 || r.height <= 0.0 {
            continue;
        }
        let self_order = doc_order.get(&n).copied().unwrap_or(usize::MAX);
        let mut floor = 0.0f32;
        for pf in floats {
            if doc_order.get(&pf.node).copied().unwrap_or(0) >= self_order {
                continue; // only earlier floats constrain
            }
            if !in_chain(parent_of, n, pf.cb) {
                continue;
            }
            let h_overlap = pf.rect.x + pf.rect.width > r.x + 0.5
                && pf.rect.x < r.x + r.width - 0.5;
            let v_overlap = pf.rect.y < r.y + r.height && pf.rect.y + pf.rect.height > r.y;
            let bottom = pf.rect.y + pf.rect.height;
            if wants_clear {
                let side_match = match style.clear {
                    ClearSide::Left => pf.left,
                    ClearSide::Right => !pf.left,
                    ClearSide::Both => true,
                    ClearSide::None => false,
                };
                // clear ignores horizontal position (CSS 2.1 §9.5.2)
                if side_match && v_overlap {
                    floor = floor.max(bottom);
                }
            }
            if is_bfc && h_overlap && v_overlap && pf.rect.y < r.y + r.height {
                // a BFC box must not overlap floats at all
                floor = floor.max(bottom);
            }
        }
        if floor > r.y + 0.5 {
            let dy = floor - r.y;
            shift_subtree(rects, children_of, n, 0.0, dy, 0);
            if let Some(&parent) = parent_of.get(&n) {
                if let Some(sibs) = children_of.get(&parent) {
                    let mut after = false;
                    for &s in sibs {
                        if s == n {
                            after = true;
                            continue;
                        }
                        if after {
                            shift_subtree(rects, children_of, s, 0.0, dy, 0);
                        }
                    }
                }
                let mut cur = Some(parent);
                while let Some(a) = cur {
                    if let Some(ar) = rects.get_mut(&a) {
                        ar.height += dy;
                    }
                    cur = parent_of.get(&a).copied();
                }
            }
        }
    }
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
