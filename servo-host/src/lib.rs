//! servo-host — the Servo-backed engine host for brows12.
//!
//! Embeds the Servo 0.6.0 engine (Stylo CSS, parallel layout, WebRender
//! paint, SpiderMonkey 153 JS) behind the brows12 shell and headless
//! harness. See docs/SERVO_INTEGRATION_PLAN.md for decisions D1-D7.

pub mod budget;
pub mod capture;
pub mod dashboard;
pub mod delegate;
pub mod headless;
pub mod memory;
pub mod metrics;
pub mod perf;
pub mod prefs;
pub mod privacy;
pub mod psi;
pub mod tabstats;
pub mod waker;

pub use budget::{
    decide as decide_degradation, tab_budget_kb, tab_estimate_kb, total_budget_kb, Degradation,
    BudgetConfig, JsHeapTier,
};
pub use dashboard::{
    dashboard_json, snapshot as dashboard_snapshot, CookieDecisionSink, CookieSection,
    PrivacyDashboard,
};
pub use headless::{run_headless, HeadlessConfig, HeadlessReport};
pub use psi::{
    psi_level, read_psi, read_system_psi, PsiConfig, PsiSnapshot,
};

/// Install a process-level rustls CryptoProvider exactly once.
///
/// ureq (DoH client) and Servo's net stack both use rustls 0.23; with more
/// than one provider feature in the graph rustls refuses to auto-pick one
/// and panics on first TLS use. Installing explicitly (aws-lc-rs, already
/// in the tree for Servo) fixes both.
pub fn init_crypto_provider() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let _ = rustls::crypto::CryptoProvider::install_default(
            rustls::crypto::aws_lc_rs::default_provider(),
        );
    });
}
pub use memory::{pressure, GovernorConfig, Pressure, SuspendedTab};
pub use metrics::{snapshot, CpuMeter, MetricsSnapshot};
pub use prefs::{brows12_preferences, compat_preferences};
