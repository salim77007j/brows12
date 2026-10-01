//! # brows12-api
//!
//! The public, documented API surface that UI layers build against.
//!
//! Two integration styles are supported:
//!
//! 1. **Rust (recommended)** — embed the engine directly:
//!
//! ```no_run
//! use brows12_api::prelude::*;
//!
//! # fn main() -> Result<(), brows12_api::ApiError> {
//! let browser = Browser::builder()
//!     .viewport(1280, 800)
//!     .privacy(|p| p.block_ads(true).https_upgrade(true))
//!     .build();
//!
//! let events = browser.subscribe();
//! let tab = browser.new_tab();
//! tab.load_url("https://example.com")?;
//!
//! // Paint the latest frame wherever you like.
//! if let Some(frame) = tab.frame() {
//!     let rgba = frame.rgba_premultiplied();
//!     println!("frame {}x{} gen {}", frame.width, frame.height, frame.generation);
//! }
//! # Ok(())
//! # }
//! ```
//!
//! 2. **C ABI (feature `capi`)** — for UIs in other languages; see
//! `api/include/brows12.h` and docs/API.md.

#[cfg(feature = "capi")]
pub mod ffi;

pub use brows12_engine::{
    Engine, EngineConfig, EngineError, EngineEvent, Frame, PrivacySettings, Tab,
};

/// The prelude collects the types a UI layer needs daily.
pub mod prelude {
    pub use crate::{Browser, BrowserBuilder, ApiError};
    pub use brows12_engine::{Engine, EngineConfig, EngineEvent, Frame, PrivacySettings, Tab};
}

use thiserror::Error;

#[allow(unused_imports)]
use brows12_layout as layout_shim;

#[derive(Debug, Error)]
pub enum ApiError {
    #[error("engine error: {0}")]
    Engine(#[from] brows12_engine::EngineError),
    #[error("invalid configuration: {0}")]
    Config(String),
}

/// High-level browser facade: engine + tab management with a builder API.
#[derive(Clone)]
pub struct Browser {
    engine: Engine,
}

/// Builder for [`Browser`].
pub struct BrowserBuilder {
    config: EngineConfig,
    privacy_fn: Box<dyn FnOnce(&mut PrivacySettings) + Send>,
    viewport: (u32, u32),
}

impl Default for BrowserBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl BrowserBuilder {
    pub fn new() -> Self {
        BrowserBuilder {
            config: EngineConfig::default(),
            privacy_fn: Box::new(|_| {}),
            viewport: (1280, 720),
        }
    }

    /// Set the viewport in CSS pixels.
    pub fn viewport(mut self, width: u32, height: u32) -> Self {
        self.viewport = (width, height);
        self
    }

    /// Adjust privacy settings.
    pub fn privacy<F>(mut self, f: F) -> Self
    where
        F: FnOnce(&mut PrivacySettings) + Send + 'static,
    {
        self.privacy_fn = Box::new(f);
        self
    }

    /// Set the user agent string.
    pub fn user_agent(mut self, ua: impl Into<String>) -> Self {
        self.config.user_agent = ua.into();
        self
    }

    /// Maximum number of simultaneously resident pages (LRU suspension).
    pub fn max_live_pages(mut self, n: usize) -> Self {
        self.config.max_live_pages = n;
        self
    }

    /// Build the browser.
    pub fn build(self) -> Browser {
        let mut config = self.config;
        (self.privacy_fn)(&mut config.privacy);
        config.viewport = layout_shim::Viewport {
            width: self.viewport.0 as f32,
            height: self.viewport.1 as f32,
        };
        Browser { engine: Engine::new(config) }
    }
}

impl Browser {
    pub fn builder() -> BrowserBuilder {
        BrowserBuilder::new()
    }

    pub fn from_engine(engine: Engine) -> Self {
        Browser { engine }
    }

    pub fn engine(&self) -> &Engine {
        &self.engine
    }

    pub fn new_tab(&self) -> std::sync::Arc<Tab> {
        self.engine.tab()
    }

    pub fn subscribe(&self) -> tokio_broadcast::Receiver<EngineEvent> {
        self.engine.subscribe()
    }
}

/// Re-export of the broadcast receiver type for signatures.
pub mod tokio_broadcast {
    pub type Receiver<T> = tokio::sync::broadcast::Receiver<T>;
    pub use tokio::sync::broadcast::error::RecvError;
}
