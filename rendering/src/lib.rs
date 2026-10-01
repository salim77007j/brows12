//! # brows12-render
//!
//! Painting pipeline for the Brows12 engine.
//!
//! * [`display_list`] — immutable, cached display lists built from the DOM,
//!   computed styles and layout result (tree order = paint order).
//! * [`raster`] — `tiny-skia` CPU rasterizer with `cosmic-text`/swash glyph
//!   atlas; identical output across platforms, zero GPU driver variance.
//! * [`compositor`] — layer stack compositing (offset + opacity), the seam
//!   where GPU acceleration (vello/wgpu) plugs in later.
//!
//! The final product is a premultiplied-RGBA framebuffer handed to the UI
//! layer plus a PNG exporter for tests and benchmarks.

pub mod compositor;
pub mod display_list;
pub mod raster;

pub use compositor::{composite, Layer};
pub use display_list::{build_display_list, DisplayItem, DisplayList, TextStyle};
pub use raster::{Rasterizer, RasterStats};

use thiserror::Error;

#[derive(Debug, Error)]
pub enum RenderError {
    #[error("image decode error: {0}")]
    Image(String),
    #[error("pixmap error: {0}")]
    Pixmap(String),
}
