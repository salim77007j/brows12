//! # brows12-privacy
//!
//! Privacy core for the Brows12 engine:
//!
//! * [`blocker`] — network-layer ad & tracker blocking (Brave's `adblock`
//!   engine, uBlock-Origin-compatible filter syntax, EasyList/EasyPrivacy).
//! * [`fingerprint`] — anti-fingerprinting configuration and the value sets
//!   injected into JS realms (canvas noise, spoofed navigator, screen, etc).
//! * [`upgrade`] — HTTPS upgrade + HSTS cache.
//! * [`policy`] — cookie policy, WebRTC leak policy, CNAME cloaking guard.

pub mod blocker;
pub mod fingerprint;
pub mod policy;
pub mod upgrade;

pub use blocker::{BlockReason, PrivacyBlocker};
pub use fingerprint::{FingerprintConfig, NavigatorSpoof, SpoofLevel};
pub use policy::{CnameVerdict, CookiePolicy, PolicyEngine, WebRtcPolicy};
pub use upgrade::HttpsUpgrader;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum PrivacyError {
    #[error("invalid filter list: {0}")]
    FilterList(String),
}

/// Aggregated stats reported to the UI (shield counters).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PrivacyStats {
    pub ads_blocked: u64,
    pub trackers_blocked: u64,
    pub upgrades_https: u64,
    pub cname_cloaks_detected: u64,
    pub fingerprint_attempts_perturbed: u64,
}
