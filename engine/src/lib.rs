//! # brows12-engine
//!
//! The Brows12 core engine: orchestrates networking, parsing, styling,
//! layout, rendering, JavaScript and storage into a coherent, tab-based
//! browsing model with privacy baked in.
//!
//! The pipeline for one navigation:
//!
//! ```text
//! URL ─▶ HTTPS upgrade ─▶ privacy filter ─▶ HTTP fetch (rustls, h1/h2)
//!     ─▶ HTML parse (html5ever) ─▶ DOM (arena)
//!     ─▶ CSS fetch/parse (lightningcss) ─▶ cascade
//!     ─▶ layout (taffy + cosmic-text) ─▶ display list
//!     ─▶ raster (tiny-skia) ─▶ framebuffer ─▶ UI
//!     ─▶ scripts (QuickJS-ng realm) ─▶ DOM mutations ─▶ re-style/re-layout
//! ```
//!
//! Tabs are cheap to create; idle tabs are suspended aggressively (their DOM,
//! style and layout trees are dropped, only the URL + snapshot remain), which
//! is the cornerstone of the engine's low idle-RAM story.

pub mod config;
pub mod fonts;
pub mod engine;
pub mod events;
pub mod tab;

pub use config::{EngineConfig, PrivacySettings};
pub use engine::Engine;
pub use events::EngineEvent;
pub use tab::{Frame, Tab};

use thiserror::Error;

#[derive(Debug, Error)]
pub enum EngineError {
    #[error("network: {0}")]
    Net(#[from] brows12_net::NetError),
    #[error("javascript: {0}")]
    Js(#[from] brows12_js::JsError),
    #[error("render: {0}")]
    Render(#[from] brows12_render::RenderError),
    #[error("page not loaded")]
    NoPage,
    #[error("navigation cancelled: {0}")]
    Cancelled(String),
    #[error("compositor: {0}")]
    Compositor(String),
}
