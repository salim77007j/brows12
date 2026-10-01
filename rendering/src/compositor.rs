//! Layer compositing — the seam where GPU acceleration will plug in.

use tiny_skia::Pixmap;

/// One composited layer: a framebuffer plus transform state.
pub struct Layer {
    pub pixmap: Pixmap,
    pub offset: (f32, f32),
    pub opacity: f32,
}

impl Layer {
    pub fn new(pixmap: Pixmap) -> Self {
        Layer { pixmap, offset: (0.0, 0.0), opacity: 1.0 }
    }

    pub fn with_offset(mut self, x: f32, y: f32) -> Self {
        self.offset = (x, y);
        self
    }

    pub fn with_opacity(mut self, opacity: f32) -> Self {
        self.opacity = opacity.clamp(0.0, 1.0);
        self
    }
}

/// Composite layers bottom-to-top into a single framebuffer.
pub fn composite(width: u32, height: u32, layers: &[Layer]) -> Option<Pixmap> {
    let mut out = Pixmap::new(width, height)?;
    for layer in layers {
        let paint = tiny_skia::PixmapPaint {
            opacity: layer.opacity.clamp(0.0, 1.0),
            blend_mode: tiny_skia::BlendMode::SourceOver,
            quality: tiny_skia::FilterQuality::Bilinear,
        };
        out.draw_pixmap(
            layer.offset.0 as i32,
            layer.offset.1 as i32,
            layer.pixmap.as_ref(),
            &paint,
            tiny_skia::Transform::identity(),
            None,
        );
    }
    Some(out)
}

/// Encode a framebuffer as PNG (tests, snapshots, benchmarks).
pub fn encode_png(pixmap: &Pixmap) -> Result<Vec<u8>, crate::RenderError> {
    let data =
        pixmap.clone().encode_png().map_err(|e| crate::RenderError::Pixmap(e.to_string()))?;
    Ok(data)
}
