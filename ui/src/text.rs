//! Chrome text rendering: cosmic-text shaping + swash glyph images blitted
//! into tiny-skia pixmaps. Replaces the v1 engine rasterizer with the same
//! two operations the chrome needs: `rasterize_text` (pixmap) and
//! `measure` (advance width).

use cosmic_text::{Attrs, Buffer, Metrics, Shaping, Style, SwashCache};
use tiny_skia::{Color, Pixmap, PremultipliedColorU8};

pub struct UiText {
    font_system: cosmic_text::FontSystem,
    swash: SwashCache,
}

impl Default for UiText {
    fn default() -> Self {
        Self::new()
    }
}

impl UiText {
    pub fn new() -> Self {
        Self { font_system: cosmic_text::FontSystem::new(), swash: SwashCache::new() }
    }

    fn shaped_width(
        &mut self,
        s: &str,
        size: f32,
        weight: u16,
        italic: bool,
    ) -> (f32, Vec<(i32, i32, cosmic_text::SwashImage)>) {
        let metrics = Metrics::new(size, size * 1.3);
        let mut buffer = Buffer::new(&mut self.font_system, metrics);
        let mut attrs = Attrs::new().weight(cosmic_text::Weight(weight));
        if italic {
            attrs = attrs.style(Style::Italic);
        }
        buffer.set_text(s, &attrs, Shaping::Advanced, None);
        buffer.shape_until_scroll(&mut self.font_system, false);

        let mut width = 0.0f32;
        let mut glyphs = Vec::new();
        for run in buffer.layout_runs() {
            for glyph in run.glyphs {
                let physical = glyph.physical((0.0, run.line_y), 1.0);
                if let Some(img) =
                    self.swash.get_image(&mut self.font_system, physical.cache_key).clone()
                {
                    width = width.max(glyph.x + glyph.w);
                    glyphs.push((physical.x, physical.y, img));
                }
            }
        }
        (width, glyphs)
    }

    /// Rasterize text into a tightly-fitted RGBA pixmap.
    #[allow(clippy::too_many_arguments)]
    pub fn rasterize_text(
        &mut self,
        s: &str,
        size: f32,
        weight: u16,
        italic: bool,
        _family: Option<&str>,
        color: [u8; 4],
        max_w: Option<f32>,
    ) -> Option<Pixmap> {
        if s.is_empty() {
            return None;
        }
        // Truncate to max width by character if needed (cheap + stable).
        let mut text = s.to_string();
        let (mut w, mut glyphs) = self.shaped_width(&text, size, weight, italic);
        if let Some(max) = max_w {
            while w > max && text.chars().count() > 1 {
                text.pop();
                let (w2, g2) = self.shaped_width(&text, size, weight, italic);
                w = w2;
                glyphs = g2;
            }
        }
        if glyphs.is_empty() {
            return None;
        }
        // Pixmap sized to glyph bounding box.
        let mut min_x = i32::MAX;
        let mut min_y = i32::MAX;
        let mut max_x = i32::MIN;
        let mut max_y = i32::MIN;
        for (gx, gy, img) in &glyphs {
            let p = img.placement;
            min_x = min_x.min(gx + p.left);
            min_y = min_y.min(gy - p.top);
            max_x = max_x.max(gx + p.left + p.width as i32);
            max_y = max_y.max(gy - p.top + p.height as i32);
        }
        let (ox, oy) = (-min_x, -min_y);
        let w_px = (max_x - min_x).max(1) as u32;
        let h_px = (max_y - min_y).max(1) as u32;
        let mut px = Pixmap::new(w_px, h_px)?;
        for (gx, gy, img) in &glyphs {
            let p = img.placement;
            if img.data.is_empty() || p.width == 0 || p.height == 0 {
                continue;
            }
            for (idx, a) in img.data.iter().enumerate() {
                // SwashImage data is a single alpha channel for mask images.
                let alpha = *a;
                if alpha == 0 {
                    continue;
                }
                let pw = p.width as usize;
                let gx_pix = gx + p.left + (idx % pw) as i32 + ox;
                let gy_pix = gy - p.top + (idx / pw) as i32 + oy;
                if gx_pix < 0 || gy_pix < 0 || gx_pix >= w_px as i32 || gy_pix >= h_px as i32 {
                    continue;
                }
                let di = gy_pix as usize * w_px as usize + gx_pix as usize;
                let dst = &mut px.pixels_mut()[di];
                let src_c = Color::from_rgba8(color[0], color[1], color[2], alpha);
                let d = dst.demultiply();
                let sa = alpha as f32 / 255.0;
                let da = d.alpha() as f32 / 255.0;
                let out_a = sa + da * (1.0 - sa);
                if out_a <= 0.0 {
                    continue;
                }
                let blend = |s: f32, d: f32| ((s * sa + d * da * (1.0 - sa)) / out_a).round() as u8;
                let r = blend(src_c.red(), f32::from(d.red()));
                let g = blend(src_c.green(), f32::from(d.green()));
                let b = blend(src_c.blue(), f32::from(d.blue()));
                *dst = PremultipliedColorU8::from_rgba(r, g, b, (out_a * 255.0).round() as u8)
                    .unwrap_or(*dst);
            }
        }
        Some(px)
    }

    /// Advance width of shaped text at `size`.
    pub fn measure(&mut self, s: &str, size: f32) -> f32 {
        if s.is_empty() {
            return 0.0;
        }
        let (w, _) = self.shaped_width(s, size, 400, false);
        w
    }
}
