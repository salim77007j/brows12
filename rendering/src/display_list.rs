//! Display list construction: the paint-order flat representation.

use brows12_css::values::Rgba;
use brows12_html::{Document, NodeData, NodeId};
use brows12_layout::Rect;
use std::collections::HashMap;
use std::sync::Arc;

/// Resolved text drawing parameters.
#[derive(Debug, Clone, PartialEq)]
pub struct TextStyle {
    pub font_size: f32,
    pub line_height_px: f32,
    pub font_weight: u16,
    pub italic: bool,
    pub font_family: Option<String>,
    pub color: Rgba,
    pub white_space_pre: bool,
    pub align: brows12_css::values::TextAlign,
}

/// One paint operation.
#[derive(Debug, Clone)]
pub enum DisplayItem {
    /// Solid rectangle fill (backgrounds).
    Rect { rect: Rect, color: Rgba, radius: f32 },
    /// Rectangle outline (borders).
    Border {
        rect: Rect,
        widths: (f32, f32, f32, f32), // top right bottom left
        color: Rgba,
        radius: f32,
    },
    /// A shaped-and-wrapped text run anchored in a box.
    Text { rect: Rect, text: String, style: TextStyle, underline: bool, line_through: bool },
    /// A decoded image blit.
    Image { rect: Rect, image: Arc<DecodedImage>, radius: f32 },
}

/// A decoded RGBA image, shared across frames.
#[derive(Debug)]
pub struct DecodedImage {
    pub width: u32,
    pub height: u32,
    /// Straight-alpha RGBA8.
    pub pixels: Vec<u8>,
}

/// Ordered paint operations for one frame.
#[derive(Debug, Clone, Default)]
pub struct DisplayList {
    pub items: Vec<DisplayItem>,
    pub viewport: (f32, f32),
}

/// Which part of the tree a display list covers (compositor layer split).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ListScope {
    /// Everything (legacy single-layer path).
    #[default]
    All,
    /// Only content that scrolls (fixed subtrees excluded).
    Content,
    /// Only `position: fixed` subtrees (compositor overlay layers).
    Fixed,
}

/// Build the display list for a document. Tree order defines paint order
/// (ancestors painted before descendants). `scroll_y` shifts document
/// content up (compositor scroll); `position: fixed` subtrees stay put.
pub fn build_display_list(
    doc: &Document,
    styles: &brows12_css::StyleMap,
    layout: &brows12_layout::LayoutResult,
    viewport: (f32, f32),
    images: &HashMap<NodeId, Arc<DecodedImage>>,
    scroll_y: f32,
    scope: ListScope,
) -> DisplayList {
    let mut list = DisplayList { items: Vec::new(), viewport };

    // Subtrees rooted at fixed nodes ignore the scroll offset.
    let mut fixed_subtrees: std::collections::HashSet<NodeId> = Default::default();
    for &root in &layout.fixed_nodes {
        fn collect_fixed(doc: &Document, node: NodeId, out: &mut std::collections::HashSet<NodeId>) {
            out.insert(node);
            for &c in &doc.node(node).children {
                collect_fixed(doc, c, out);
            }
        }
        collect_fixed(doc, root, &mut fixed_subtrees);
    }

    let start = doc.body().or_else(|| doc.document_element()).unwrap_or(doc.root());

    #[allow(clippy::too_many_arguments)]
    fn emit(
        doc: &Document,
        styles: &brows12_css::StyleMap,
        layout: &brows12_layout::LayoutResult,
        list: &mut DisplayList,
        images: &HashMap<NodeId, Arc<DecodedImage>>,
        scroll_y: f32,
        fixed_subtrees: &std::collections::HashSet<NodeId>,
        is_fixed: bool,
        node: NodeId,
        scope: ListScope,
    ) {
        let Some(style) = styles.get(node) else {
            return;
        };
        let in_fixed_subtree = is_fixed || fixed_subtrees.contains(&node);
        match scope {
            ListScope::All => {}
            ListScope::Content => {
                if in_fixed_subtree {
                    return;
                }
            }
            ListScope::Fixed => {
                if !in_fixed_subtree {
                    return;
                }
            }
        }
        if style.display == brows12_css::values::Display::None {
            return;
        }
        let Some(mut rect) = layout.rect(node) else {
            return;
        };
        // Compositor scroll: content moves up; fixed layers stay anchored.
        if !is_fixed && scroll_y != 0.0 {
            rect.y -= scroll_y;
        }

        match &doc.node(node).data {
            NodeData::Element { .. } => {
                let child_fixed = in_fixed_subtree;
                // Background
                if style.background_color[3] > 0 {
                    list.items.push(DisplayItem::Rect {
                        rect,
                        color: style.background_color,
                        radius: style.border_radius,
                    });
                }
                // Border
                let bw = &style.border_width;
                let total: f32 = bw.top.extract_px()
                    + bw.right.extract_px()
                    + bw.bottom.extract_px()
                    + bw.left.extract_px();
                if total > 0.0 {
                    list.items.push(DisplayItem::Border {
                        rect,
                        widths: (
                            bw.top.extract_px(),
                            bw.right.extract_px(),
                            bw.bottom.extract_px(),
                            bw.left.extract_px(),
                        ),
                        color: style.border_color,
                        radius: style.border_radius,
                    });
                }
                // Image content
                if let Some(img) = images.get(&node) {
                    list.items.push(DisplayItem::Image {
                        rect,
                        image: img.clone(),
                        radius: style.border_radius,
                    });
                }
                for &c in &doc.node(node).children {
                    emit(
                        doc, styles, layout, list, images, scroll_y, fixed_subtrees, child_fixed, c,
                        scope,
                    );
                }
            }
            NodeData::Text(_) => {
                if style.display == brows12_css::values::Display::None || rect.height <= 0.0 {
                    return;
                }
                let text_style = TextStyle {
                    font_size: style.font_size,
                    line_height_px: style.line_height_px(),
                    font_weight: style.font_weight,
                    italic: style.font_style == brows12_css::values::FontStyle::Italic,
                    font_family: style.font_family.clone(),
                    color: style.color,
                    white_space_pre: style.white_space == brows12_css::values::WhiteSpace::Pre,
                    align: style.text_align,
                };
                let text = doc.text_content(node);
                list.items.push(DisplayItem::Text {
                    rect,
                    text,
                    style: text_style,
                    underline: style.text_underline,
                    line_through: style.text_line_through,
                });
            }
            _ => {
                for &c in &doc.node(node).children {
                    emit(
                        doc, styles, layout, list, images, scroll_y, fixed_subtrees, is_fixed, c,
                        scope,
                    );
                }
            }
        }
    }

    emit(
        doc, styles, layout, &mut list, images, scroll_y, &fixed_subtrees, false, start, scope,
    );
    list
}

trait ExtractPx {
    fn extract_px(&self) -> f32;
}

impl ExtractPx for brows12_css::values::Len {
    fn extract_px(&self) -> f32 {
        match self {
            brows12_css::values::Len::Px(px) => *px,
            brows12_css::values::Len::Percent(_) => 0.0,
        }
    }
}
