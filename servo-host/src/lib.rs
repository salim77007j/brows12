//! servo-host — the Servo-backed engine host for brows12.
//!
//! Embeds the Servo 0.6.0 engine (Stylo CSS, parallel layout, WebRender
//! paint, SpiderMonkey 153 JS) behind the brows12 shell and headless
//! harness. See docs/SERVO_INTEGRATION_PLAN.md for decisions D1-D7.

pub mod capture;
pub mod delegate;
pub mod headless;
pub mod prefs;
pub mod privacy;
pub mod waker;

pub use headless::{run_headless, HeadlessConfig, HeadlessReport};
pub use prefs::compat_preferences;
