//! The brows12 memory governor: policy knobs and pressure levels for
//! keeping resident memory inside a budget by hibernating background
//! tabs (Phase 2.1).
//!
//! Division of labour:
//! - this module: pure policy (no engine types, unit-testable);
//! - the shell (`ui`): owns the tabs, executes the policy
//!   (`hibernate` = throttle + hide + drop the WebView, which sends
//!   CloseWebView and frees the document pipeline);
//! - `servo.create_memory_report()`: engine-side accounting used by the
//!   brows-perf harness for the per-subsystem breakdown.

use std::time::Duration;

/// Governor configuration, environment-tunable so CI and the automation
/// harness can exercise every policy path without rebuilds.
#[derive(Debug, Clone)]
pub struct GovernorConfig {
    /// Process RSS budget. Above it the governor hibernates the oldest
    /// background tab, repeating until under budget or out of candidates.
    /// Env: `BROWS12_MEM_BUDGET_MB` (default 384).
    pub budget_kb: u64,
    /// Master switch. Env: `BROWS12_GOVERNOR` ("0" disables).
    pub enabled: bool,
    /// How often the governor samples RSS while the shell idles.
    /// Env: `BROWS12_GOVERNOR_INTERVAL_MS` (default 5000).
    pub interval: Duration,
    /// A background tab becomes a hibernation candidate after being
    /// inactive this long (protects alt-tab flaps).
    /// Env: `BROWS12_TAB_SUSPEND_SECS` (default 180).
    pub suspend_after: Duration,
    /// A tab whose page issued at least this many resource requests is
    /// classified HEAVY (proxy for large DOM + asset count on ~100MB-class
    /// sites). Heavy tabs suspend far sooner in the background.
    /// Env: `BROWS12_HEAVY_REQUESTS` (default 60; 0 disables weighting).
    pub heavy_page_requests: u64,
    /// Hibernation delay for HEAVY background tabs.
    /// Env: `BROWS12_HEAVY_SUSPEND_SECS` (default 20).
    pub heavy_suspend_after: Duration,
    /// Phase 4.4.1: delay before the FIRST governor tick (warmup), so
    /// session setup (tab opening, activation history) is complete
    /// before the predictive reclaim starts ranking. 0 in production.
    /// Env: `BROWS12_GOVERNOR_WARMUP_MS` (default 0).
    pub warmup: Duration,
}

impl Default for GovernorConfig {
    fn default() -> Self {
        GovernorConfig {
            budget_kb: 384 * 1024,
            enabled: true,
            interval: Duration::from_millis(5_000),
            suspend_after: Duration::from_secs(180),
            heavy_page_requests: 60,
            heavy_suspend_after: Duration::from_secs(20),
            warmup: Duration::ZERO,
        }
    }
}

impl GovernorConfig {
    pub fn from_env() -> Self {
        let mut cfg = GovernorConfig::default();
        if let Ok(v) = std::env::var("BROWS12_MEM_BUDGET_MB") {
            if let Ok(mb) = v.parse::<u64>() {
                cfg.budget_kb = mb * 1024;
            }
        }
        if let Ok(v) = std::env::var("BROWS12_GOVERNOR") {
            cfg.enabled = v != "0";
        }
        if let Ok(v) = std::env::var("BROWS12_GOVERNOR_INTERVAL_MS") {
            if let Ok(ms) = v.parse::<u64>() {
                cfg.interval = Duration::from_millis(ms.max(250));
            }
        }
        if let Ok(v) = std::env::var("BROWS12_TAB_SUSPEND_SECS") {
            if let Ok(s) = v.parse::<u64>() {
                cfg.suspend_after = Duration::from_secs(s);
            }
        }
        if let Ok(v) = std::env::var("BROWS12_HEAVY_REQUESTS") {
            if let Ok(n) = v.parse::<u64>() {
                cfg.heavy_page_requests = n;
            }
        }
        if let Ok(v) = std::env::var("BROWS12_HEAVY_SUSPEND_SECS") {
            if let Ok(s) = v.parse::<u64>() {
                cfg.heavy_suspend_after = Duration::from_secs(s);
            }
        }
        if let Ok(v) = std::env::var("BROWS12_GOVERNOR_WARMUP_MS") {
            if let Ok(ms) = v.parse::<u64>() {
                cfg.warmup = Duration::from_millis(ms);
            }
        }
        cfg
    }

    pub fn budget_mb(&self) -> u64 {
        self.budget_kb / 1024
    }
}

/// Memory pressure level derived from the current RSS.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pressure {
    /// Under 80% of budget — no action.
    Nominal,
    /// 80-99% of budget — stop promoting new work to caches (reserved).
    Elevated,
    /// At or over budget — hibernate background tabs now.
    Critical,
}

pub fn pressure(cfg: &GovernorConfig, rss_kb: u64) -> Pressure {
    if !cfg.enabled {
        return Pressure::Nominal;
    }
    let ratio = rss_kb as f64 / cfg.budget_kb as f64;
    if ratio >= 1.0 {
        Pressure::Critical
    } else if ratio >= 0.8 {
        Pressure::Elevated
    } else {
        Pressure::Nominal
    }
}

/// Phase 3.9 — how long a background tab with this page weight waits
/// before it is a hibernation candidate. Heavy pages (many resources:
/// large DOMs, big image sets) give up their pipeline much sooner than
/// light pages, which keeps ~100MB-class sites from dominating RSS.
pub fn suspend_delay(page_requests: u64, cfg: &GovernorConfig) -> Duration {
    let heavy = cfg.heavy_page_requests > 0 && page_requests >= cfg.heavy_page_requests;
    if heavy {
        cfg.heavy_suspend_after
    } else {
        cfg.suspend_after
    }
}

/// Phase 3.9 — whether a tab is hibernation-eligible under the current
/// pressure level. At `Elevated` only HEAVY tabs are reclaimed (light
/// tabs keep their pipelines); at `Critical` everything eligible goes.
pub fn eligible_under_pressure(
    page_requests: u64,
    inactive_for: Duration,
    level: Pressure,
    cfg: &GovernorConfig,
) -> bool {
    if inactive_for < suspend_delay(page_requests, cfg) {
        return false;
    }
    match level {
        Pressure::Nominal => false,
        Pressure::Elevated => {
            cfg.heavy_page_requests > 0 && page_requests >= cfg.heavy_page_requests
        }
        Pressure::Critical => true,
    }
}

/// Snapshot of a tab's identity kept when its WebView is dropped
/// (hibernation). Restoration rebuilds the WebView and reloads —
/// engines cannot freeze a live document without process snapshotting,
/// and the mission target for a suspended tab (< 20 MB RSS marginal)
/// is only reachable by actually freeing the pipeline.
#[derive(Debug, Clone)]
pub struct SuspendedTab {
    /// URL to reload on restore.
    pub url: String,
    /// Title to keep showing on the tab strip.
    pub title: String,
    /// When the tab became inactive (governor eligibility).
    pub inactive_since: std::time::Instant,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pressure_levels() {
        let cfg = GovernorConfig { budget_kb: 1000, ..Default::default() };
        assert_eq!(pressure(&cfg, 700), Pressure::Nominal);
        assert_eq!(pressure(&cfg, 850), Pressure::Elevated);
        assert_eq!(pressure(&cfg, 1000), Pressure::Critical);
        let off = GovernorConfig { enabled: false, ..cfg };
        assert_eq!(pressure(&off, 5000), Pressure::Nominal);
    }

    #[test]
    fn env_overrides() {
        // SAFETY: tests in this crate run single-threaded per process
        // scope for these vars (cargo test harness serializes env use is
        // NOT guaranteed — unique var names per test to be safe).
        std::env::set_var("BROWS12_MEM_BUDGET_MB", "100");
        let cfg = GovernorConfig::from_env();
        assert_eq!(cfg.budget_kb, 100 * 1024);
        std::env::remove_var("BROWS12_MEM_BUDGET_MB");
    }

    #[test]
    fn heavy_page_policy() {
        let cfg = GovernorConfig {
            suspend_after: Duration::from_secs(180),
            heavy_page_requests: 60,
            heavy_suspend_after: Duration::from_secs(20),
            ..Default::default()
        };
        assert_eq!(suspend_delay(10, &cfg), Duration::from_secs(180));
        assert_eq!(suspend_delay(60, &cfg), Duration::from_secs(20));
        assert_eq!(suspend_delay(5000, &cfg), Duration::from_secs(20));
        let no_weight = GovernorConfig { heavy_page_requests: 0, ..cfg };
        assert_eq!(suspend_delay(5000, &no_weight), Duration::from_secs(180));
    }

    #[test]
    fn elevated_pressures_only_heavy() {
        let cfg = GovernorConfig {
            suspend_after: Duration::from_secs(180),
            heavy_page_requests: 60,
            heavy_suspend_after: Duration::from_secs(20),
            ..Default::default()
        };
        let young_heavy = Duration::from_secs(10);
        let old_heavy = Duration::from_secs(25);
        let old_light = Duration::from_secs(200);
        // Elevated: heavy + old enough → eligible; light or young → not.
        assert!(eligible_under_pressure(100, old_heavy, Pressure::Elevated, &cfg));
        assert!(!eligible_under_pressure(100, young_heavy, Pressure::Elevated, &cfg));
        assert!(!eligible_under_pressure(10, old_light, Pressure::Elevated, &cfg));
        // Critical: everything past its (weight-dependent) delay is eligible.
        assert!(eligible_under_pressure(10, old_light, Pressure::Critical, &cfg));
        assert!(eligible_under_pressure(100, old_heavy, Pressure::Critical, &cfg));
        assert!(!eligible_under_pressure(10, young_heavy, Pressure::Critical, &cfg));
        assert!(!eligible_under_pressure(10, old_light, Pressure::Nominal, &cfg));
    }
}
