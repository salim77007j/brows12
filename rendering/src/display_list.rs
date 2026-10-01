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

/// Build the display list for a document. Tree order defines paint order
/// (ancestors painted before descendants).
pub fn build_display_list(
    doc: &Document,
    styles: &brows12_css::StyleMap,
    layout: &brows12_layout::LayoutResult,
    viewport: (f32, f32),
    images: &HashMap<NodeId, Arc<DecodedImage>>,
) -> DisplayList {
    let mut list = DisplayList { items: Vec::new(), viewport };

    let start = doc.body().or_else(|| doc.document_element()).unwrap_or(doc.root());

    fn emit(
        doc: &Document,
        styles: &brows12_css::StyleMap,
        layout: &brows12_layout::LayoutResult,
        list: &mut DisplayList,
        images: &HashMap<NodeId, Arc<DecodedImage>>,
        node: NodeId,
    ) {
        let Some(style) = styles.get(node) else {
            return;
        };
        if style.display == brows12_css::values::Display::None {
            return;
        }
        let Some(rect) = layout.rect(node) else {
            return;
        };

        match &doc.node(node).data {
            NodeData::Element { name, .. } => {
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
                let total: f32 = bw.top.extract_px() + bw.right.extract_px() + bw.bottom.extract_px() + bw.left.extract_px();
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
                let _ = name;
                for &c in &doc.node(node).children {
                    emit(doc, styles, layout, list, images, c);
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
                    emit(doc, styles, layout, list, images, c);
                }
            }
        }
    }

    emit(doc, styles, layout, &mut list, images, start);
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

