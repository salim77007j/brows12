//! CPU rasterizer: tiny-skia surfaces + swash glyph blitting.

use crate::display_list::{DisplayItem, DisplayList};
use crate::RenderError;
use std::sync::{Arc, Mutex};
use tiny_skia::{Color, Paint, PathBuilder, Pixmap, Rect as SkRect};

/// Counters from one paint pass (telemetry for benchmarks).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct RasterStats {
    pub rects: u32,
    pub borders: u32,
    pub text_runs: u32,
    pub images: u32,
}

/// Shared font/glyph infrastructure (owned by the engine).
pub struct Rasterizer {
    pub font_system: Arc<Mutex<cosmic_text::FontSystem>>,
    pub swash_cache: Mutex<cosmic_text::SwashCache>,
}

impl Rasterizer {
    pub fn new(font_system: Arc<Mutex<cosmic_text::FontSystem>>) -> Self {
        Self { font_system, swash_cache: Mutex::new(cosmic_text::SwashCache::new()) }
    }

    /// Rasterize a display list into a fresh framebuffer.
    pub fn paint(
        &mut self,
        list: &DisplayList,
        width: u32,
        height: u32,
    ) -> Result<(Pixmap, RasterStats), RenderError> {
        let mut pixmap = Pixmap::new(width, height)
            .ok_or_else(|| RenderError::Pixmap("invalid surface size".into()))?;
        let mut stats = RasterStats::default();
        for item in &list.items {
            match item {
                DisplayItem::Rect { rect, color, radius } => {
                    fill_rect(&mut pixmap, rect, *color, *radius);
                    stats.rects += 1;
                }
                DisplayItem::Border { rect, widths, color, radius } => {
                    stroke_border(&mut pixmap, rect, *widths, *color, *radius);
                    stats.borders += 1;
                }
                DisplayItem::Text { rect, text, style, underline, line_through } => {
                    self.paint_text(&mut pixmap, *rect, text, style, *underline, *line_through);
                    stats.text_runs += 1;
                }
                DisplayItem::Image { rect, image, radius } => {
                    draw_image(&mut pixmap, rect, image, *radius);
                    stats.images += 1;
                }
            }
        }
        Ok((pixmap, stats))
    }

    /// Render text into a tightly-fitted premultiplied pixmap.
    /// Public seam for Canvas2D `fillText` (js crate) and tests.
    pub fn rasterize_text(
        &mut self,
        text: &str,
        font_size: f32,
        font_weight: u16,
        italic: bool,
        font_family: Option<&str>,
        color: [u8; 4],
        max_width: Option<f32>,
    ) -> Option<Pixmap> {
        // Measure first (same shaping path as layout).
        let measure_style = crate::display_list::TextStyle {
            font_size,
            line_height_px: font_size * 1.2,
            font_weight,
            italic,
            font_family: font_family.map(|s| s.to_string()),
            color,
            white_space_pre: false,
            align: brows12_css::values::TextAlign::Start,
        };
        let mut measurer = brows12_layout::TextMeasurer::new(self.font_system.clone());
        let leaf = brows12_layout::LeafContext::Text {
            text: text.to_string(),
            font_size,
            line_height_px: font_size * 1.2,
            font_weight,
            font_style_italic: italic,
            font_family: font_family.map(|s| s.to_string()),
            white_space_pre: false,
        };
        let (w, h) = measurer.measure(&leaf, max_width);
        let pw = (w.ceil() as u32).max(1);
        let ph = (h.ceil() as u32).max(1);
        let mut pm = Pixmap::new(pw, ph)?;
        let rect = brows12_layout::Rect { x: 0.0, y: 0.0, width: pw as f32, height: ph as f32 };
        self.paint_text(&mut pm, rect, text, &measure_style, false, false);
        Some(pm)
    }

    /// Shape + blit a text run with swash images (alpha coverage tinted by
    /// the style color, or full-color images for emoji).
    fn paint_text(
        &mut self,
        pixmap: &mut Pixmap,
        rect: brows12_layout::Rect,
        text: &str,
        style: &crate::display_list::TextStyle,
        underline: bool,
        line_through: bool,
    ) {
        let mut fs = self.font_system.lock().unwrap();
        let mut cache = self.swash_cache.lock().unwrap();

        let metrics = cosmic_text::Metrics::new(style.font_size, style.line_height_px);
        let mut buffer = cosmic_text::Buffer::new(&mut fs, metrics);
        buffer.set_size(Some(rect.width.max(1.0)), None);

        let mut attrs = cosmic_text::Attrs::new();
        attrs = match style.font_family.as_deref() {
            Some("monospace") => attrs.family(cosmic_text::Family::Monospace),
            Some("serif") => attrs.family(cosmic_text::Family::Serif),
            Some(name) => attrs.family(cosmic_text::Family::Name(name)),
            None => attrs.family(cosmic_text::Family::SansSerif),
        };
        attrs = attrs.weight(cosmic_text::Weight(style.font_weight));
        if style.italic {
            attrs = attrs.style(cosmic_text::Style::Italic);
        }
        buffer.set_text(text, &attrs, cosmic_text::Shaping::Advanced, None);
        buffer.shape_until_scroll(&mut fs, false);

        let tint =
            tiny_skia::Color::from_rgba8(style.color[0], style.color[1], style.color[2], 255);

        for run in buffer.layout_runs() {
            // Align lines horizontally per text-align.
            let line_x = match style.align {
                brows12_css::values::TextAlign::Center => {
                    rect.x + (rect.width - run.line_w).max(0.0) / 2.0
                }
                brows12_css::values::TextAlign::Right => {
                    rect.x + (rect.width - run.line_w).max(0.0)
                }
                _ => rect.x,
            };
            for glyph in run.glyphs {
                let physical = glyph.physical((line_x, run.line_y), 1.0);
                if let Some(image) = cache.get_image(&mut fs, physical.cache_key) {
                    let x = physical.x + image.placement.left;
                    let y = physical.y + image.placement.top;
                    blit_swash_image(pixmap, image, x, y, tint);
                }
            }
            // Underline / line-through rectangles.
            let mut deco = |dy: f32| {
                let y = (rect.y + run.line_y - dy) as i32;
                let w = run.line_w.ceil() as u32;
                fill_pixel_rect(
                    pixmap,
                    line_x as i32,
                    y,
                    w,
                    1.max((style.font_size / 16.0) as u32),
                    tint,
                );
            };
            if underline {
                deco(style.font_size * 0.12);
            }
            if line_through {
                deco(style.font_size * 0.30);
            }
        }
    }
}

/// Fill an axis-aligned (optionally rounded) rect with premultiplied blending.
fn fill_rect(pixmap: &mut Pixmap, rect: &brows12_layout::Rect, color: [u8; 4], radius: f32) {
    if color[3] == 0 {
        return;
    }
    let sk_color = Color::from_rgba8(color[0], color[1], color[2], color[3]);
    let mut paint = Paint::default();
    paint.set_color(sk_color);
    paint.anti_alias = true;

    let Some(sk_rect) =
        SkRect::from_xywh(rect.x, rect.y, rect.width.max(0.0), rect.height.max(0.0))
    else {
        return;
    };
    if radius > 0.0 {
        // Rounded rect via 4-corner curves (tiny-skia-path has no
        // push_round_rect; manual construction keeps quality identical).
        let r = radius.min(rect.width / 2.0).min(rect.height / 2.0);
        let mut pb = PathBuilder::new();
        pb.move_to(rect.x + r, rect.y);
        pb.line_to(rect.x + rect.width - r, rect.y);
        pb.quad_to(rect.x + rect.width, rect.y, rect.x + rect.width, rect.y + r);
        pb.line_to(rect.x + rect.width, rect.y + rect.height - r);
        pb.quad_to(
            rect.x + rect.width,
            rect.y + rect.height,
            rect.x + rect.width - r,
            rect.y + rect.height,
        );
        pb.line_to(rect.x + r, rect.y + rect.height);
        pb.quad_to(rect.x, rect.y + rect.height, rect.x, rect.y + rect.height - r);
        pb.line_to(rect.x, rect.y + r);
        pb.quad_to(rect.x, rect.y, rect.x + r, rect.y);
        pb.close();
        if let Some(path) = pb.finish() {
            pixmap.fill_path(
                &path,
                &paint,
                tiny_skia::FillRule::Winding,
                tiny_skia::Transform::identity(),
                None,
            );
        }
    } else {
        pixmap.fill_rect(sk_rect, &paint, tiny_skia::Transform::identity(), None);
    }
}

/// Draw a border as four filled strips (v1 approximation of per-side widths).
fn stroke_border(
    pixmap: &mut Pixmap,
    rect: &brows12_layout::Rect,
    widths: (f32, f32, f32, f32),
    color: [u8; 4],
    _radius: f32,
) {
    let (t, r, b, l) = widths;
    if color[3] == 0 {
        return;
    }
    let mut strip = |rect: brows12_layout::Rect| {
        if rect.width > 0.0 && rect.height > 0.0 {
            fill_rect(pixmap, &rect, color, 0.0);
        }
    };
    strip(brows12_layout::Rect { x: rect.x, y: rect.y, width: rect.width, height: t }); // top
    strip(brows12_layout::Rect {
        x: rect.x + rect.width - r,
        y: rect.y,
        width: r,
        height: rect.height,
    }); // right
    strip(brows12_layout::Rect {
        x: rect.x,
        y: rect.y + rect.height - b,
        width: rect.width,
        height: b,
    }); // bottom
    strip(brows12_layout::Rect { x: rect.x, y: rect.y, width: l, height: rect.height });
    // left
}

fn fill_pixel_rect(pixmap: &mut Pixmap, x: i32, y: i32, w: u32, h: u32, color: tiny_skia::Color) {
    if let Some(sk_rect) = SkRect::from_xywh(x as f32, y as f32, w as f32, h as f32) {
        let mut paint = Paint::default();
        paint.set_color(color);
        paint.anti_alias = false;
        pixmap.fill_rect(sk_rect, &paint, tiny_skia::Transform::identity(), None);
    }
}

/// Blit one swash glyph image: alpha coverage tinted with the text color, or
/// premultiplied RGBA for color glyphs (emoji).
fn blit_swash_image(
    pixmap: &mut Pixmap,
    image: &cosmic_text::SwashImage,
    x: i32,
    y: i32,
    tint: tiny_skia::Color,
) {
    use cosmic_text::SwashContent;
    let w = image.placement.width;
    let h = image.placement.height;
    if w == 0 || h == 0 {
        return;
    }
    let x = x + image.placement.left;
    let y = y + image.placement.top;

    if image.content == SwashContent::Color {
        // RGBA8 straight-alpha → premultiply into an offscreen pixmap and draw.
        if let Some(mut glyph_pixmap) = Pixmap::new(w, h) {
            let mut premul = Vec::with_capacity((w * h * 4) as usize);
            for px in image.data.chunks_exact(4) {
                let a = px[3] as u16;
                premul.push((px[0] as u16 * a / 255) as u8);
                premul.push((px[1] as u16 * a / 255) as u8);
                premul.push((px[2] as u16 * a / 255) as u8);
                premul.push(px[3]);
            }
            glyph_pixmap.data_mut().copy_from_slice(&premul);
            pixmap.draw_pixmap(
                x,
                y,
                glyph_pixmap.as_ref(),
                &tiny_skia::PixmapPaint::default(),
                tiny_skia::Transform::identity(),
                None,
            );
        }
    } else {
        // Alpha mask: blend coverage with the tint color manually.
        let (tr, tg, tb, ta) = (tint.red(), tint.green(), tint.blue(), tint.alpha());
        let pw = pixmap.width() as i32;
        let ph = pixmap.height() as i32;
        let pm = pixmap.pixels_mut();
        for row in 0..h as i32 {
            let py = y + row;
            if py < 0 || py >= ph {
                continue;
            }
            for col in 0..w as i32 {
                let px = x + col;
                if px < 0 || px >= pw {
                    continue;
                }
                let coverage = image.data[(row * w as i32 + col) as usize] as f32 / 255.0;
                if coverage <= 0.0 {
                    continue;
                }
                let idx = (py * pw + px) as usize;
                let dst = &mut pm[idx];
                // src-over with premultiplied src
                let sa = coverage * ta;
                let out_a = sa + dst.alpha() as f32 / 255.0 * (1.0 - sa);
                if out_a <= 0.0 {
                    continue;
                }
                let blend = |s: f32, d: f32| {
                    (s * sa + d * (dst.alpha() as f32 / 255.0) * (1.0 - sa)) / out_a
                };
                let r = blend(tr, dst.red() as f32 / 255.0);
                let g = blend(tg, dst.green() as f32 / 255.0);
                let b = blend(tb, dst.blue() as f32 / 255.0);
                *dst = tiny_skia::PremultipliedColorU8::from_rgba(
                    (r * 255.0).round() as u8,
                    (g * 255.0).round() as u8,
                    (b * 255.0).round() as u8,
                    (out_a * 255.0).round() as u8,
                )
                .unwrap_or(*dst);
            }
        }
    }
}

/// Decode + scale an image into the pixmap (box-filter nearest for v1).
fn draw_image(
    pixmap: &mut Pixmap,
    rect: &brows12_layout::Rect,
    image: &crate::display_list::DecodedImage,
    _radius: f32,
) {
    let Some(mut img_pixmap) = Pixmap::new(image.width, image.height) else {
        return;
    };
    // Straight → premultiplied alpha.
    let mut premul = Vec::with_capacity(image.pixels.len());
    for px in image.pixels.chunks_exact(4) {
        let a = px[3] as u16;
        premul.push((px[0] as u16 * a / 255) as u8);
        premul.push((px[1] as u16 * a / 255) as u8);
        premul.push((px[2] as u16 * a / 255) as u8);
        premul.push(px[3]);
    }
    img_pixmap.data_mut().copy_from_slice(&premul);

    // Scale to the destination rect (bilinear via tiny-skia when available).
    let scaled = scale_pixmap(&img_pixmap, rect.width.max(1.0) as u32, rect.height.max(1.0) as u32);
    if let Some(scaled) = scaled {
        pixmap.draw_pixmap(
            rect.x as i32,
            rect.y as i32,
            scaled.as_ref(),
            &tiny_skia::PixmapPaint::default(),
            tiny_skia::Transform::identity(),
            None,
        );
    }
}

/// Pixmap scaling (tiny-skia 0.12 exposes resize behind a different path;
/// this bilinear implementation keeps rendering dependency-light).
fn scale_pixmap(src: &Pixmap, w: u32, h: u32) -> Option<Pixmap> {
    if w == src.width() && h == src.height() {
        return Some(src.clone());
    }
    let mut out = Pixmap::new(w, h)?;
    let sw = src.width() as f32;
    let sh = src.height() as f32;
    let dst = out.pixels_mut();
    let spx = src.pixels();
    for row in 0..h {
        let sy = (row as f32 + 0.5) / h as f32 * sh;
        let y0 = (sy as usize).min(src.height() as usize - 1);
        for col in 0..w {
            let sx = (col as f32 + 0.5) / w as f32 * sw;
            let x0 = (sx as usize).min(src.width() as usize - 1);
            dst[(row * w + col) as usize] = spx[y0 * src.width() as usize + x0];
        }
    }
    Some(out)
}

/// Decode raw image bytes (png/jpeg/webp/gif) into straight-alpha RGBA.
pub fn decode_image(bytes: &[u8]) -> Result<crate::display_list::DecodedImage, RenderError> {
    let img =
        image::load_from_memory(bytes).map_err(|e| RenderError::Image(e.to_string()))?.to_rgba8();
    Ok(crate::display_list::DecodedImage {
        width: img.width(),
        height: img.height(),
        pixels: img.into_raw(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compositor::encode_png;
    use crate::display_list::{build_display_list, DisplayItem, TextStyle};
    use brows12_layout::Rect;
    use std::collections::HashMap;

    fn measurer() -> Rasterizer {
        Rasterizer::new(Arc::new(Mutex::new(cosmic_text::FontSystem::new())))
    }

    #[test]
    fn fills_background() {
        let mut r = measurer();
        let list = DisplayList {
            items: vec![DisplayItem::Rect {
                rect: Rect { x: 10.0, y: 10.0, width: 100.0, height: 50.0 },
                color: [255, 0, 0, 255],
                radius: 0.0,
            }],
            viewport: (200.0, 200.0),
        };
        let (pixmap, stats) = r.paint(&list, 200, 200).unwrap();
        assert_eq!(stats.rects, 1);
        let px = pixmap.pixel(50, 30).unwrap();
        assert_eq!(px.red(), 255);
        assert_eq!(px.green(), 0);
    }

    #[test]
    fn paints_text_ink() {
        let mut r = measurer();
        let style = TextStyle {
            font_size: 32.0,
            line_height_px: 38.0,
            font_weight: 400,
            italic: false,
            font_family: None,
            color: [0, 0, 0, 255],
            white_space_pre: false,
            align: brows12_css::values::TextAlign::Start,
        };
        let list = DisplayList {
            items: vec![DisplayItem::Text {
                rect: Rect { x: 20.0, y: 20.0, width: 400.0, height: 50.0 },
                text: "Hello Brows12".to_string(),
                style,
                underline: false,
                line_through: false,
            }],
            viewport: (600.0, 200.0),
        };
        let (pixmap, stats) = r.paint(&list, 600, 200).unwrap();
        assert_eq!(stats.text_runs, 1);
        // Text must have inked some pixels.
        let inked = pixmap.pixels().iter().filter(|p| p.alpha() > 0).count();
        assert!(inked > 10, "expected text ink, got {} px", inked);
        let png = encode_png(&pixmap).unwrap();
        assert!(png.starts_with(&[0x89, b'P', b'N', b'G']));
    }

    #[test]
    fn full_pipeline_end_to_end() {
        let doc = brows12_html::parse_document(
            "<html><body><h1 style=\"color:#ff0000\">Render me</h1><p>Body text here.</p></body></html>",
        );
        let engine = brows12_css::StyleEngine::with_author_sheets(&[]);
        let ctx = brows12_css::computed::CascadeCtx::default();
        let styles = brows12_css::compute_styles(&doc, &engine, &ctx);
        let measurer = Arc::new(brows12_layout::TextMeasurer::new(Arc::new(Mutex::new(
            cosmic_text::FontSystem::new(),
        ))));
        let layout = brows12_layout::compute_layout(
            &doc,
            &styles,
            brows12_layout::Viewport::default(),
            &measurer,
            &HashMap::new(),
        );
        let list = build_display_list(
            &doc, &styles, &layout, (1280.0, 720.0), &HashMap::new(), 0.0,
            Default::default(),
        );
        assert!(list.items.iter().any(|i| matches!(i, DisplayItem::Text { .. })));
        let mut r = Rasterizer::new(measurer.font_system.clone());
        let (pixmap, _stats) = r.paint(&list, 1280, 720).unwrap();
        let inked = pixmap.pixels().iter().filter(|p| p.alpha() > 0).count();
        assert!(inked > 100, "page must render visible ink, got {} px", inked);
    }
}
