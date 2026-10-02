//! # brows12-compositor
//!
//! Layer compositing for Brows12 — the GPU-accelerated seam between the
//! raster pipeline and the screen.
//!
//! Two backends implement the same contract:
//! - **GPU** (feature `gpu`, default): `wgpu` render pipeline. Layers upload
//!   as textures; scroll, transforms and opacity are applied per draw call,
//!   so scrolling never re-rasterizes content. Works headless (offscreen
//!   texture + readback) and with any Vulkan/Metal/DX12/GL adapter.
//! - **CPU**: tiny-skia composition with the same layer semantics — the
//!   battery-saver and no-GPU fallback, and the deterministic reference
//!   the GPU path is validated against.
//!
//! The engine decides at runtime: try GPU, fall back to CPU, and report
//! which backend produced each frame (see `CompositeStats`).

mod cpu;
pub mod layer;

#[cfg(feature = "gpu")]
mod gpu;

pub use cpu::CpuCompositor;
pub use layer::{Layer, Transform2D};

#[cfg(feature = "gpu")]
pub use gpu::GpuCompositor;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum CompositorError {
    #[error("no layers to composite")]
    Empty,
    #[error("GPU backend unavailable: {0}")]
    #[cfg(feature = "gpu")]
    Gpu(String),
    #[error("layer size exceeds limits")]
    TooLarge,
}

/// Which backend produced a frame.
#[derive(Debug, Clone, PartialEq)]
pub enum Backend {
    /// wgpu adapter name + backend API.
    Gpu { adapter: String, api: String },
    /// tiny-skia software composition.
    Cpu,
}

/// Measured composite statistics for one frame.
#[derive(Debug, Clone, PartialEq)]
pub struct CompositeStats {
    pub backend: Backend,
    /// Wall-clock composite time in microseconds.
    pub frame_time_us: f64,
    /// Number of layers composited.
    pub layer_count: usize,
    /// Bytes of layer texture memory uploaded/resident.
    pub texture_bytes: usize,
}

/// Output of a composite: straight-alpha RGBA8 pixels + stats.
pub struct CompositeOutput {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
    pub stats: CompositeStats,
}

impl CompositeOutput {
    /// Wrap pixels into a tiny-skia pixmap (premultiplied conversion done by
    /// tiny-skia) for the engine's Frame plumbing.
    pub fn to_pixmap(&self) -> Option<tiny_skia::Pixmap> {
        let mut pm = tiny_skia::Pixmap::new(self.width, self.height)?;
        // GPU/CPU outputs are straight alpha; tiny-skia frames are
        // premultiplied — convert here.
        for (dst, px) in pm.pixels_mut().iter_mut().zip(self.pixels.chunks_exact(4)) {
            let a = px[3] as u16;
            let premul = |c: u8| ((c as u16 * a + 127) / 255) as u8;
            *dst =
                tiny_skia::ColorU8::from_rgba(premul(px[0]), premul(px[1]), premul(px[2]), px[3])
                    .premultiply();
        }
        Some(pm)
    }
}

/// A shared rendering contract for both backends.
pub trait Compositor {
    fn composite(
        &self,
        width: u32,
        height: u32,
        layers: &[Layer],
    ) -> Result<CompositeOutput, CompositorError>;
}

/// Auto-select: GPU when an adapter is available, CPU otherwise.
pub fn auto_compositor() -> Box<dyn Compositor> {
    #[cfg(feature = "gpu")]
    {
        match GpuCompositor::new() {
            Ok(gpu) => {
                tracing::info!(adapter = %gpu.adapter_name(), "compositor: GPU backend");
                Box::new(gpu)
            }
            Err(e) => {
                tracing::info!(error = %e, "compositor: GPU unavailable, CPU fallback");
                Box::new(CpuCompositor::new())
            }
        }
    }
    #[cfg(not(feature = "gpu"))]
    {
        Box::new(CpuCompositor::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(w: u32, h: u32, color: [u8; 4]) -> Layer {
        let mut pm = tiny_skia::Pixmap::new(w, h).unwrap();
        let c = tiny_skia::Color::from_rgba8(color[0], color[1], color[2], color[3]);
        pm.fill(c);
        Layer::from_pixmap(pm)
    }

    fn roundtrip(c: &dyn Compositor) {
        let red = solid(64, 64, [255, 0, 0, 255]);
        let blue = solid(32, 32, [0, 0, 255, 255]).with_offset(16.0, 16.0);
        let out = c.composite(64, 64, &[red, blue]).unwrap();
        assert_eq!(out.width, 64);
        // Center pixel blends to a blue-over-red mix with alpha 255.
        let idx = ((32 * 64 + 32) * 4) as usize;
        let px = &out.pixels[idx..idx + 4];
        assert_eq!(px[3], 255);
        assert!(px[2] > px[0], "blue should dominate over red at center: {px:?}");
    }

    #[test]
    fn cpu_composite_blends_layers() {
        roundtrip(&CpuCompositor::new());
    }

    #[cfg(feature = "gpu")]
    #[test]
    fn gpu_composite_matches_cpu_when_adapter_present() {
        let Ok(gpu) = GpuCompositor::new() else {
            eprintln!("no GPU adapter in this environment; skipping GPU roundtrip");
            return;
        };
        roundtrip(&gpu);
    }
}
