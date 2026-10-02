//! CPU composite backend (tiny-skia): the deterministic fallback.

use crate::layer::Layer;
use crate::{CompositeOutput, Compositor, CompositorError};

/// Software compositor. Deterministic, allocation-frugal, runs everywhere.
#[derive(Debug, Default, Clone, Copy)]
pub struct CpuCompositor;

impl CpuCompositor {
    pub fn new() -> Self {
        CpuCompositor
    }
}

/// Un-premultiply premultiplied RGBA8 into straight alpha (lossy on low
/// alpha values, standard for CPU/GPU handoff).
pub(crate) fn premul_to_straight(data: &[u8]) -> Vec<u8> {
    let mut out = data.to_vec();
    for px in out.chunks_exact_mut(4) {
        let a = px[3];
        if a == 0 {
            px[0] = 0;
            px[1] = 0;
            px[2] = 0;
        } else if a != 255 {
            let a16 = a as u16;
            px[0] = ((px[0] as u16 * 255 + a16 / 2) / a16).min(255) as u8;
            px[1] = ((px[1] as u16 * 255 + a16 / 2) / a16).min(255) as u8;
            px[2] = ((px[2] as u16 * 255 + a16 / 2) / a16).min(255) as u8;
        }
    }
    out
}

impl Compositor for CpuCompositor {
    fn composite(
        &self,
        width: u32,
        height: u32,
        layers: &[Layer],
    ) -> Result<CompositeOutput, CompositorError> {
        let t0 = std::time::Instant::now();
        if layers.is_empty() {
            return Err(CompositorError::Empty);
        }
        let mut surface = tiny_skia::Pixmap::new(width, height).ok_or(CompositorError::TooLarge)?;
        let paint = tiny_skia::PixmapPaint {
            opacity: 1.0,
            blend_mode: tiny_skia::BlendMode::SourceOver,
            quality: tiny_skia::FilterQuality::Bilinear,
        };
        for layer in layers {
            let (ox, oy) = layer.effective_offset((0.0, 0.0));
            let m = layer.transform.matrix();
            let ts = tiny_skia::Transform::from_row(m[0], m[1], m[2], m[3], m[4], m[5])
                .pre_translate(ox, oy);
            // Per-layer opacity goes through PixmapPaint; transform+scroll
            // through the draw transform — mirrors the GPU uniform split.
            let layer_paint = tiny_skia::PixmapPaint {
                opacity: layer.opacity.clamp(0.0, 1.0),
                blend_mode: tiny_skia::BlendMode::SourceOver,
                quality: tiny_skia::FilterQuality::Bilinear,
            };
            surface.draw_pixmap(0, 0, layer.pixmap.as_ref(), &layer_paint, ts, None);
        }
        let _ = paint;
        let stats = crate::CompositeStats {
            backend: crate::Backend::Cpu,
            frame_time_us: t0.elapsed().as_secs_f64() * 1e6,
            layer_count: layers.len(),
            texture_bytes: layers.iter().map(Layer::texture_bytes).sum(),
        };
        Ok(CompositeOutput { width, height, pixels: premul_to_straight(surface.data()), stats })
    }
}
