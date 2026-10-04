//! Privacy dashboard — Phase 4 Focus Area 2.8 (data layer only).
//!
//! Aggregates every Area 2 counter into one serializable snapshot the
//! future UI can render (and the JSON report already embeds):
//!
//! * ads / trackers blocked (2.1) + `$redirect` / `$removeparam` /
//!   `$csp` / scriptlet / cosmetic activity,
//! * anti-fingerprinting coverage (2.2),
//! * pop-ups + interstitial redirect chains blocked (2.3),
//! * HTTPS upgrades + HSTS hits (2.4) and runtime-learned HSTS (2.7),
//! * DoH queries + CNAME cloaks detected (2.5),
//! * cookie decisions: partitioned / force-partitioned / rejected (2.6
//!   decision layer; engine-side wiring recorded as upstream work),
//! * security guard: header probes, frames blocked, CSP violations,
//!   mixed-content blocks, COOP/COEP/CORP observations (2.7).
//!
//! UI rendering is intentionally out of scope here: this module only
//! owns the numbers and their JSON shape.

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use crate::privacy::PrivacyHost;

/// Cookie decision counters (fed by the 2.6 decision layer).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CookieSection {
    /// Third-party cookies stored under their own `Partitioned` key.
    pub partitioned_opt_in: u64,
    /// Third-party cookies force-partitioned under the top-level site.
    pub force_partitioned: u64,
    /// Third-party Set-Cookie headers dropped.
    pub rejected: u64,
    /// `__Host-` violations dropped.
    pub invalid_host_prefix: u64,
}

/// One dashboard snapshot. All fields are counts since browser start.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrivacyDashboard {
    pub ads_blocked: u64,
    pub trackers_blocked: u64,
    /// Blocked-request sample (capped 512).
    pub blocked_sample: Vec<String>,
    /// Filter rules loaded into the matcher.
    pub rules_loaded: usize,
    /// Requests answered with a `$redirect` replacement resource.
    pub redirects_served: u64,
    /// Requests rewritten via `$removeparam`.
    pub params_stripped: u64,
    /// `$csp` directive sets surfaced for documents/iframes.
    pub csp_injections: u64,
    /// Pages that received uBO scriptlet code.
    pub scriptlets_injected: u64,
    /// Pages filtered by phase-2 class/id cosmetic matching.
    pub cosmetic_pages_filtered: u64,
    /// Pages protected by the anti-fingerprinting defense (2.2).
    pub fingerprint_pages_protected: u64,
    /// Pop-ups (window.open / pop-unders) blocked (2.3).
    pub popups_blocked: u64,
    /// Interstitial redirect chains cut off (2.3).
    pub redirect_chains_blocked: u64,
    /// http→https navigations/subresources upgraded (2.4).
    pub https_upgrades: u64,
    /// HSTS enforcement hits (preload + cache) (2.4).
    pub hsts_hits: u64,
    /// HSTS policies learned at runtime from response headers (2.7).
    pub hsts_learned: u64,
    /// DoH queries issued by the browser itself (2.5).
    pub doh_queries: u64,
    /// CNAME-cloaked hosts detected (2.5).
    pub cname_cloaks: u64,
    /// Cookie decisions (2.6 decision layer).
    pub cookies: CookieSection,
    /// Response-header security guard counters (2.7).
    pub security_probes: u64,
    pub frames_blocked: u64,
    pub csp_blocked: u64,
    pub mixed_content_blocked: u64,
    pub coop_observed: u64,
    pub coep_observed: u64,
    pub corp_observed: u64,
    /// Decision logs (capped) for the dashboard detail views.
    pub frame_log: Vec<String>,
    pub csp_block_log: Vec<String>,
    pub popup_log: Vec<String>,
    pub cloak_log: Vec<String>,
}

/// Cookie decision accounting on the dashboard side. The 2.6 decision
/// layer returns a [`brows12_storage::CookieDecision`] per evaluated
/// Set-Cookie; the embedder calls [`record_cookie_decision`] until the
/// engine-side jar wiring lands (upstream follow-up).
#[derive(Default)]
pub struct CookieDecisionSink {
    pub partitioned_opt_in: Mutex<u64>,
    pub force_partitioned: Mutex<u64>,
    pub rejected: Mutex<u64>,
    pub invalid_host_prefix: Mutex<u64>,
}

impl CookieDecisionSink {
    pub fn record(&self, d: brows12_storage::CookieDecision) {
        use brows12_storage::CookieDecision as D;
        let slot = match d {
            D::PartitionedOptIn => &self.partitioned_opt_in,
            D::ForcePartitioned => &self.force_partitioned,
            D::RejectedThirdParty => &self.rejected,
            D::InvalidHostPrefix => &self.invalid_host_prefix,
            D::Passthrough => return,
        };
        *slot.lock().unwrap() += 1;
    }

    pub fn section(&self) -> CookieSection {
        CookieSection {
            partitioned_opt_in: *self.partitioned_opt_in.lock().unwrap(),
            force_partitioned: *self.force_partitioned.lock().unwrap(),
            rejected: *self.rejected.lock().unwrap(),
            invalid_host_prefix: *self.invalid_host_prefix.lock().unwrap(),
        }
    }
}

/// Build the dashboard snapshot from the live privacy host.
pub fn snapshot(privacy: &Arc<PrivacyHost>) -> PrivacyDashboard {
    let (ads_blocked, trackers_blocked, blocked_sample) = privacy.blocked_summary();
    let sec = privacy.security.summary();
    PrivacyDashboard {
        ads_blocked,
        trackers_blocked,
        blocked_sample,
        rules_loaded: privacy.blocker.rules_loaded,
        redirects_served: privacy.redirects_served.load(Ordering::Relaxed),
        params_stripped: privacy.params_stripped.load(Ordering::Relaxed),
        csp_injections: privacy.csp_injections.load(Ordering::Relaxed),
        scriptlets_injected: privacy.scriptlets_injected.load(Ordering::Relaxed),
        cosmetic_pages_filtered: privacy.cosmetic_pages_filtered.load(Ordering::Relaxed),
        fingerprint_pages_protected: privacy.fingerprint_pages_protected.load(Ordering::Relaxed),
        popups_blocked: privacy.popups_blocked.load(Ordering::Relaxed),
        redirect_chains_blocked: privacy.redirect_chains_blocked.load(Ordering::Relaxed),
        https_upgrades: privacy.upgrader.upgrade_count(),
        hsts_hits: privacy.upgrader.hsts_hit_count(),
        hsts_learned: sec.hsts_learned,
        doh_queries: privacy.doh.query_count(),
        cname_cloaks: privacy.cname_cloaks.load(Ordering::Relaxed),
        cookies: privacy.cookie_sink.section(),
        security_probes: sec.probes,
        frames_blocked: sec.frames_blocked,
        csp_blocked: sec.csp_blocked,
        mixed_content_blocked: sec.mixed_content_blocked,
        coop_observed: sec.coop_observed,
        coep_observed: sec.coep_observed,
        corp_observed: sec.corp_observed,
        frame_log: sec.frame_log,
        csp_block_log: sec.csp_block_log,
        popup_log: privacy.popup_log.lock().unwrap().clone(),
        cloak_log: privacy.cloak_log.lock().unwrap().clone(),
    }
}

/// The dashboard as compact JSON (the API surface for the future UI).
pub fn dashboard_json(privacy: &Arc<PrivacyHost>) -> String {
    serde_json::to_string(&snapshot(privacy)).unwrap_or_else(|_| "{}".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use brows12_storage::CookieDecision;

    #[test]
    fn snapshot_reflects_live_counters() {
        let privacy = PrivacyHost::new();
        privacy.ads_blocked.store(3, Ordering::Relaxed);
        privacy.trackers_blocked.store(5, Ordering::Relaxed);
        privacy.record_popup_blocked("https://pop.example/a");
        privacy.record_popup_blocked("https://ad.example/");
        let dash = snapshot(&privacy);
        assert_eq!(dash.ads_blocked, 3);
        assert_eq!(dash.trackers_blocked, 5);
        assert_eq!(dash.popups_blocked, 2);
        assert!(dash.popup_log.iter().any(|l| l.contains("ad.example")));
    }

    #[test]
    fn cookie_sink_accounts_decisions() {
        let sink = CookieDecisionSink::default();
        sink.record(CookieDecision::ForcePartitioned);
        sink.record(CookieDecision::ForcePartitioned);
        sink.record(CookieDecision::PartitionedOptIn);
        sink.record(CookieDecision::RejectedThirdParty);
        sink.record(CookieDecision::InvalidHostPrefix);
        sink.record(CookieDecision::Passthrough); // not counted
        let s = sink.section();
        assert_eq!(s.force_partitioned, 2);
        assert_eq!(s.partitioned_opt_in, 1);
        assert_eq!(s.rejected, 1);
        assert_eq!(s.invalid_host_prefix, 1);
    }

    #[test]
    fn dashboard_json_is_valid() {
        let privacy = PrivacyHost::new();
        let json = dashboard_json(&privacy);
        let parsed: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
        assert!(parsed["ads_blocked"].is_u64());
        assert!(parsed["cookies"]["rejected"].is_u64());
        assert!(parsed["frames_blocked"].is_u64());
    }
}
