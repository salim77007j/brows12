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
                DisplayItem::InlineFlow { rect, flow } => {
                    self.paint_inline_flow(&mut pixmap, *rect, flow);
                    stats.text_runs += 1;
                }
                DisplayItem::GradientRect { rect, gradient, radius } => {
                    fill_gradient(&mut pixmap, rect, gradient, *radius);
                    stats.rects += 1;
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
    #[allow(clippy::too_many_arguments)]
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
        let measurer = brows12_layout::TextMeasurer::new(self.font_system.clone());
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

    /// Paint a laid-out inline flow: per line, segment backgrounds, then
    /// glyphs, then decorations. Segments carry their own style; the
    /// rasterizer never re-wraps (positions come from layout).
    fn paint_inline_flow(
        &mut self,
        pixmap: &mut Pixmap,
        rect: brows12_layout::Rect,
        flow: &brows12_layout::InlineFlowLayout,
    ) {
        let mut fs = self.font_system.lock().unwrap();
        let mut cache = self.swash_cache.lock().unwrap();

        for line in &flow.lines {
            // Segment backgrounds first (behind all glyphs of the line).
            for seg in &line.segments {
                if let Some(bg) = seg.background {
                    if bg[3] > 0 {
                        fill_pixel_rect(
                            pixmap,
                            (rect.x + seg.x) as i32,
                            (rect.y + line.y) as i32,
                            seg.width.ceil() as u32,
                            line.height.ceil() as u32,
                            tiny_skia::Color::from_rgba8(bg[0], bg[1], bg[2], bg[3]),
                        );
                    }
                }
            }
            // Glyphs per segment.
            for seg in &line.segments {
                let metrics = cosmic_text::Metrics::new(seg.font_size, seg.line_height_px);
                let mut buffer = cosmic_text::Buffer::new(&mut fs, metrics);
                buffer.set_size(None, None);
                let mut attrs = cosmic_text::Attrs::new();
                attrs = match seg.font_family.as_deref() {
                    Some("monospace") => attrs.family(cosmic_text::Family::Monospace),
                    Some("serif") => attrs.family(cosmic_text::Family::Serif),
                    Some(name) => attrs.family(cosmic_text::Family::Name(name)),
                    None => attrs.family(cosmic_text::Family::SansSerif),
                };
                attrs = attrs.weight(cosmic_text::Weight(seg.font_weight));
                if seg.italic {
                    attrs = attrs.style(cosmic_text::Style::Italic);
                }
                buffer.set_text(&seg.text, &attrs, cosmic_text::Shaping::Advanced, None);
                buffer.shape_until_scroll(&mut fs, false);

                let tint =
                    tiny_skia::Color::from_rgba8(seg.color[0], seg.color[1], seg.color[2], 255);
                let origin_x = rect.x + seg.x;
                let baseline_y = rect.y + line.baseline;
                for run in buffer.layout_runs() {
                    for glyph in run.glyphs {
                        let physical = glyph.physical((origin_x, baseline_y), 1.0);
                        if let Some(image) = cache.get_image(&mut fs, physical.cache_key) {
                            let x = physical.x + image.placement.left;
                            let y = physical.y - image.placement.top;
                            blit_swash_image(pixmap, image, x, y, tint);
                        }
                    }
                }
                // Decorations for this segment.
                let thickness = ((seg.font_size / 16.0) as u32).max(1);
                if seg.underline {
                    let y = (baseline_y + seg.font_size * 0.12) as i32;
                    fill_pixel_rect(
                        pixmap,
                        origin_x as i32,
                        y,
                        seg.width.ceil() as u32,
                        thickness,
                        tint,
                    );
                }
                if seg.line_through {
                    let y = (baseline_y - seg.font_size * 0.30) as i32;
                    fill_pixel_rect(
                        pixmap,
                        origin_x as i32,
                        y,
                        seg.width.ceil() as u32,
                        thickness,
                        tint,
                    );
                }
            }
        }
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
        // +1px slack: the box width came from measuring this exact text, and
        // a borderline exact-fit must not re-wrap at paint time.
        buffer.set_size(Some(rect.width.max(1.0) + 1.0), None);

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

        if std::env::var("BROWS_DEBUG").is_ok() {
            eprintln!(
                "PAINT_TEXT rect=({:.0},{:.0} {}x{}) fs={} lh={:.1} runs:",
                rect.x, rect.y, rect.width, rect.height, style.font_size, style.line_height_px
            );
            for run in buffer.layout_runs() {
                eprintln!(
                    "  run line_y={:.1} line_h={:.1} w={:.1} glyphs={}",
                    run.line_y,
                    run.line_height,
                    run.line_w,
                    run.glyphs.len()
                );
            }
        }

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
                let physical = glyph.physical((line_x, rect.y + run.line_y), 1.0);
                if let Some(image) = cache.get_image(&mut fs, physical.cache_key) {
                    let x = physical.x + image.placement.left;
                    // swash placement.top is the distance UP from the
                    // baseline to the bitmap top: bitmap_y = baseline - top.
                    let y = physical.y - image.placement.top;
                    blit_swash_image(pixmap, image, x, y, tint);
                }
            }
            // Underline / line-through rectangles.
            // Underline sits BELOW the baseline (CSS visual convention);
            // line-through crosses the x-height middle.
            let mut deco = |dy: f32| {
                let y = (rect.y + run.line_y + dy) as i32;
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
                deco(-style.font_size * 0.30);
            }
        }
    }
}

/// Paint a CSS gradient into a rect (per-pixel projection; v1 covers the
/// linear and radial cases our parser produces).
fn fill_gradient(
    pixmap: &mut Pixmap,
    rect: &brows12_layout::Rect,
    gradient: &brows12_css::values::Gradient,
    radius: f32,
) {
    let (stops, angle_deg, radial) = match gradient {
        brows12_css::values::Gradient::Linear { angle_deg, stops } => (stops, *angle_deg, false),
        brows12_css::values::Gradient::Radial { stops } => (stops, 0.0, true),
    };
    if stops.is_empty() {
        return;
    }
    // Resolve stop positions: explicit values stand; runs of unknowns are
    // distributed evenly between the surrounding known positions (CSS rule).
    let n = stops.len();
    let mut pos = vec![0.0f32; n];
    let mut i = 0;
    while i < n {
        if stops[i].position.is_some() {
            pos[i] = stops[i].position.unwrap().clamp(0.0, 1.0);
            i += 1;
            continue;
        }
        let start = i.saturating_sub(1);
        let mut j = i;
        while j < n && stops[j].position.is_none() {
            j += 1;
        }
        let lo = if start < i { pos[start] } else { 0.0 };
        let hi = if j < n { pos[j] } else { 1.0 };
        let count = j - i;
        for k in 0..count {
            let denom = if j < n { count + 1 } else { count } as f32;
            pos[i + k] = lo + (hi - lo) * ((k + 1) as f32 / denom);
        }
        i = j;
    }

    let w = pixmap.width() as i32;
    let ph = pixmap.height() as i32;
    let x0 = rect.x.floor() as i32;
    let y0 = rect.y.floor() as i32;
    let rw = rect.width.ceil() as i32;
    let rh = rect.height.ceil() as i32;
    let cx = rect.x + rect.width / 2.0;
    let cy = rect.y + rect.height / 2.0;
    // CSS gradient angle → direction vector (0deg = up, 90deg = right).
    let rad = angle_deg.to_radians();
    let (dx, dy) = (rad.sin(), -rad.cos());
    // Gradient line length so stops span the box corners.
    let line_len = (rect.width * dx).abs() + (rect.height * dy).abs();
    let r_radial = ((rect.width / 2.0).powi(2) + (rect.height / 2.0).powi(2)).sqrt();
    let r = radius.min(rect.width / 2.0).min(rect.height / 2.0);

    let pm = pixmap.pixels_mut();
    for py in y0.max(0)..(y0 + rh).min(ph) {
        for px in x0.max(0)..(x0 + rw).min(w) {
            let fx = px as f32 + 0.5;
            let fy = py as f32 + 0.5;
            // Rounded-rect membership (when radius requested).
            if r > 0.0 {
                let qx = (fx - rect.x - r).abs().max(0.0) - (rect.width - 2.0 * r).max(0.0) / 2.0;
                let qy = (fy - rect.y - r).abs().max(0.0) - (rect.height - 2.0 * r).max(0.0) / 2.0;
                let inside = qx.max(0.0).hypot(qy.max(0.0)) <= r || (qx <= 0.0 && qy <= 0.0);
                if !inside {
                    continue;
                }
            }
            let t = if radial {
                let ddx = fx - cx;
                let ddy = fy - cy;
                (ddx * ddx + ddy * ddy).sqrt() / r_radial.max(1.0)
            } else {
                if line_len <= 0.001 {
                    0.0
                } else {
                    ((fx - cx) * dx + (fy - cy) * dy) / line_len + 0.5
                }
            };
            // Sample stops.
            let color = sample_stops(&pos, stops, t.clamp(0.0, 1.0));
            let idx = (py * w + px) as usize;
            let dst = &mut pm[idx];
            let (sr, sg, sb, sa) = (
                color[0] as f32 / 255.0,
                color[1] as f32 / 255.0,
                color[2] as f32 / 255.0,
                color[3] as f32 / 255.0,
            );
            let da = dst.alpha() as f32 / 255.0;
            let out_a = sa + da * (1.0 - sa);
            if out_a <= 0.0 {
                continue;
            }
            let blend = |s: f32, d: f32| (s * sa + d * da * (1.0 - sa)) / out_a;
            *dst = tiny_skia::PremultipliedColorU8::from_rgba(
                (blend(sr, dst.red() as f32 / 255.0) * 255.0).round() as u8,
                (blend(sg, dst.green() as f32 / 255.0) * 255.0).round() as u8,
                (blend(sb, dst.blue() as f32 / 255.0) * 255.0).round() as u8,
                (out_a * 255.0).round() as u8,
            )
            .unwrap_or(*dst);
        }
    }
}

fn sample_stops(pos: &[f32], stops: &[brows12_css::values::GradientStop], t: f32) -> [u8; 4] {
    if t <= pos[0] {
        return stops[0].color;
    }
    if t >= pos[pos.len() - 1] {
        return stops[stops.len() - 1].color;
    }
    for i in 1..pos.len() {
        if t <= pos[i] {
            let span = pos[i] - pos[i - 1];
            let f = if span <= 0.0001 { 0.0 } else { (t - pos[i - 1]) / span };
            let a = &stops[i - 1].color;
            let b = &stops[i].color;
            let lerp = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * f).round() as u8;
            return [lerp(a[0], b[0]), lerp(a[1], b[1]), lerp(a[2], b[2]), lerp(a[3], b[3])];
        }
    }
    stops[stops.len() - 1].color
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
    // NOTE: the caller has already resolved the final bitmap origin
    // (x = physical.x + placement.left, y = baseline - placement.top).
    // swash placement is applied exactly once; re-adding it here would
    // push every glyph's bitmap top onto the baseline (vertical glyph
    // jitter) and double the left side bearing (horizontal jitter).

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

/// Decode raw image bytes (png/jpeg/webp/gif/svg) into straight-alpha RGBA.
/// SVG sources rasterize through resvg at their intrinsic size; the layout
/// scales the result into the destination box.
pub fn decode_image(bytes: &[u8]) -> Result<crate::display_list::DecodedImage, RenderError> {
    // SVG sniffing: `<svg` at start, XML prolog, or `<svg` anywhere in
    // ASCII-looking content (data: image/svg+xml arrives decoded here).
    let looks_svg = bytes.starts_with(b"<svg")
        || bytes.starts_with(b"<?xml")
        || std::str::from_utf8(bytes).map(|s| s.contains("<svg")).unwrap_or(false);
    if looks_svg {
        let opt = resvg::usvg::Options::default();
        let tree = resvg::usvg::Tree::from_data(bytes, &opt)
            .map_err(|e| RenderError::Image(format!("svg: {e}")))?;
        let size = tree.size();
        let w = (size.width().ceil() as u32).max(1);
        let h = (size.height().ceil() as u32).max(1);
        let mut pm = resvg::tiny_skia::Pixmap::new(w, h)
            .ok_or_else(|| RenderError::Image("svg pixmap".into()))?;
        let sx = w as f32 / size.width().max(1.0);
        let sy = h as f32 / size.height().max(1.0);
        resvg::render(&tree, resvg::usvg::Transform::from_scale(sx, sy), &mut pm.as_mut());
        return Ok(crate::display_list::DecodedImage {
            width: w,
            height: h,
            pixels: pm.data().to_vec(),
        });
    }
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
            tagged: Vec::new(),
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
            tagged: Vec::new(),
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
            &doc,
            &styles,
            &layout,
            (1280.0, 720.0),
            &HashMap::new(),
            0.0,
            Default::default(),
        );
        assert!(
            list.items
                .iter()
                .any(|i| matches!(i, DisplayItem::Text { .. } | DisplayItem::InlineFlow { .. })),
            "page must emit text items (Text or InlineFlow), got {:?}",
            list.items
                .iter()
                .map(|i| match i {
                    DisplayItem::Rect { .. } => "rect",
                    DisplayItem::Border { .. } => "border",
                    DisplayItem::Text { .. } => "text",
                    DisplayItem::InlineFlow { .. } => "inline-flow",
                    DisplayItem::Image { .. } => "image",
                    DisplayItem::GradientRect { .. } => "gradient",
                })
                .collect::<Vec<_>>()
        );
        let mut r = Rasterizer::new(measurer.font_system.clone());
        let (pixmap, _stats) = r.paint(&list, 1280, 720).unwrap();
        let inked = pixmap.pixels().iter().filter(|p| p.alpha() > 0).count();
        assert!(inked > 100, "page must render visible ink, got {} px", inked);
    }
}
