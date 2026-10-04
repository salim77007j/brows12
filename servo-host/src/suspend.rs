//! Phase 4 Area 4.7 — configurable suspend policy for background tabs.
//!
//! Policy modes (env `BROWS12_SUSPEND_POLICY`):
//! - `never`       — no idle suspension (memory-pressure reclaims stay
//!                   active; this knob governs IDLE suspension only);
//! - `<seconds>`   — suspend a background tab after this much idle time
//!                   since the user last had it active (default 300 s);
//! - `aggressive`  — 60 s idle suspension.
//!
//! Exemptions (never suspended by the idle pass): the active tab,
//! pinned tabs (4.6), tabs currently PLAYING media (4.7, via the
//! engine's media-session playback-state events), and tabs whose URL
//! contains one of `BROWS12_SUSPEND_EXEMPT_URLS` (comma-separated
//! substrings — the user-level exception list).

use std::time::Duration;

/// Aggressive-mode idle threshold.
pub const AGGRESSIVE_IDLE: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, PartialEq)]
pub enum SuspendPolicy {
    /// No idle suspension.
    Never,
    /// Suspend after this much idle time.
    AfterIdle(Duration),
}

impl SuspendPolicy {
    pub fn from_env() -> SuspendPolicy {
        let v = std::env::var("BROWS12_SUSPEND_POLICY").unwrap_or_default();
        Self::parse(&v)
    }

    pub fn parse(v: &str) -> SuspendPolicy {
        match v.trim() {
            "" => SuspendPolicy::AfterIdle(Duration::from_secs(300)),
            "never" | "off" | "no" => SuspendPolicy::Never,
            "aggressive" | "aggro" => SuspendPolicy::AfterIdle(AGGRESSIVE_IDLE),
            other => match other.parse::<u64>() {
                // A zero/negative-ish value means "never" in practice.
                Ok(secs) if secs == 0 => SuspendPolicy::Never,
                Ok(secs) => SuspendPolicy::AfterIdle(Duration::from_secs(secs)),
                Err(_) => SuspendPolicy::AfterIdle(Duration::from_secs(300)),
            },
        }
    }

    /// The idle threshold, or None in `never` mode.
    pub fn idle_threshold(&self) -> Option<Duration> {
        match self {
            SuspendPolicy::Never => None,
            SuspendPolicy::AfterIdle(d) => Some(*d),
        }
    }
}

/// Exempt URL substrings from `BROWS12_SUSPEND_EXEMPT_URLS`.
pub fn exempt_urls_from_env() -> Vec<String> {
    std::env::var("BROWS12_SUSPEND_EXEMPT_URLS")
        .unwrap_or_default()
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Pure decision for the idle-suspend pass (governor tick, Nominal
/// pressure). `idle_for`: time since the user last had the tab active.
pub fn should_suspend(
    is_active: bool,
    pinned: bool,
    media_playing: bool,
    idle_for: Duration,
    url: &str,
    policy: &SuspendPolicy,
    exempt_urls: &[String],
) -> bool {
    if is_active || pinned || media_playing {
        return false;
    }
    if exempt_urls.iter().any(|frag| url.contains(frag.as_str())) {
        return false;
    }
    match policy.idle_threshold() {
        None => false,
        Some(threshold) => idle_for >= threshold,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> (SuspendPolicy, Vec<String>) {
        (SuspendPolicy::AfterIdle(Duration::from_secs(300)), vec![])
    }

    #[test]
    fn policy_parsing() {
        assert_eq!(SuspendPolicy::parse(""), SuspendPolicy::AfterIdle(Duration::from_secs(300)));
        assert_eq!(SuspendPolicy::parse("never"), SuspendPolicy::Never);
        assert_eq!(SuspendPolicy::parse("0"), SuspendPolicy::Never);
        assert_eq!(
            SuspendPolicy::parse("aggressive"),
            SuspendPolicy::AfterIdle(Duration::from_secs(60))
        );
        assert_eq!(
            SuspendPolicy::parse("45"),
            SuspendPolicy::AfterIdle(Duration::from_secs(45))
        );
        // Garbage falls back to the default.
        assert_eq!(
            SuspendPolicy::parse("nonsense"),
            SuspendPolicy::AfterIdle(Duration::from_secs(300))
        );
        assert_eq!(SuspendPolicy::Never.idle_threshold(), None);
    }

    #[test]
    fn idle_threshold_decides() {
        let (policy, exempt) = ctx();
        // Fresh tab: no suspension.
        assert!(!should_suspend(
            false, false, false, Duration::from_secs(10), "https://x/", &policy, &exempt
        ));
        // Past the threshold: suspend.
        assert!(should_suspend(
            false, false, false, Duration::from_secs(301), "https://x/", &policy, &exempt
        ));
        // Exactly at the threshold: suspend (>=).
        assert!(should_suspend(
            false, false, false, Duration::from_secs(300), "https://x/", &policy, &exempt
        ));
    }

    #[test]
    fn exemptions() {
        let (policy, exempt) = ctx();
        let long = Duration::from_secs(4000);
        // Active tab.
        assert!(!should_suspend(
            true, false, false, long, "https://x/", &policy, &exempt
        ));
        // Pinned tab.
        assert!(!should_suspend(
            false, true, false, long, "https://x/", &policy, &exempt
        ));
        // Playing media.
        assert!(!should_suspend(
            false, false, true, long, "https://x/", &policy, &exempt
        ));
        // URL exemption (substring).
        let (p2, e2) = (policy.clone(), vec!["music.example".to_string()]);
        assert!(!should_suspend(
            false, false, false, long, "https://music.example/listen", &p2, &e2
        ));
        // Other URLs still suspend.
        assert!(should_suspend(
            false, false, false, long, "https://news.example/", &p2, &e2
        ));
    }

    #[test]
    fn never_mode_disables_idle_suspension() {
        let (policy, exempt) = (SuspendPolicy::Never, vec![]);
        assert!(!should_suspend(
            false, false, false, Duration::from_secs(999_999), "https://x/", &policy, &exempt
        ));
    }
}
