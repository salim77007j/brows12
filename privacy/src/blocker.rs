//! Network-layer request blocking powered by Brave's `adblock` engine.
//!
//! The embedded default list is a curated subset of EasyList/EasyPrivacy
//! covering the highest-volume trackers and ad networks so the engine is
//! protected out of the box; full lists can be loaded from disk at runtime.

use crate::PrivacyStats;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::RwLock;

/// Why a request was blocked (reported to the UI).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockReason {
    /// Matched an advertising filter.
    Ad { filter: String },
    /// Matched a tracker/privacy filter.
    Tracker { filter: String },
}

/// The request types we classify (subset of adblock's categories).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestKind {
    Document,
    Script,
    Image,
    Stylesheet,
    Xhr,
    Font,
    Other,
}

impl RequestKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            RequestKind::Document => "main_frame",
            RequestKind::Script => "script",
            RequestKind::Image => "image",
            RequestKind::Stylesheet => "stylesheet",
            RequestKind::Xhr => "xhr",
            RequestKind::Font => "font",
            RequestKind::Other => "other",
        }
    }
}

pub struct PrivacyBlocker {
    engine: adblock::engine::Engine,
    stats: PrivacyStats,
    counts: BlockingCounters,
}

#[derive(Default)]
struct BlockingCounters {
    ads: AtomicU64,
    trackers: AtomicU64,
}

impl Default for PrivacyBlocker {
    fn default() -> Self {
        Self::new()
    }
}

impl PrivacyBlocker {
    /// Build a blocker with the embedded default filter list.
    pub fn new() -> Self {
        Self::from_filters(DEFAULT_FILTERS)
    }

    /// Build from a custom filter list text (EasyList syntax).
    pub fn from_filters(filters: &str) -> Self {
        let engine = adblock::engine::Engine::new_with_list_text(filters);
        Self {
            engine,
            stats: PrivacyStats::default(),
            counts: BlockingCounters::default(),
        }
    }

    /// Should this network request be blocked?
    /// `source_hostname` is the hostname of the embedding document.
    pub fn check(
        &self,
        url: &str,
        source_hostname: &str,
        kind: RequestKind,
    ) -> Option<BlockReason> {
        let request = adblock::request::Request::new(
            url,
            source_hostname,
            kind.as_str(),
            "GET",
        )
        .ok()?;
        let result = self.engine.check_network_request(&request);
        if result.should_block() {
            let filter = result
                .filter
                .map(|f| f.raw_line)
                .unwrap_or_default()
                .unwrap_or_default();
            let reason = if filter.contains("privacy") || looks_like_tracker(url) {
                self.counts.trackers.fetch_add(1, Ordering::Relaxed);
                BlockReason::Tracker { filter }
            } else {
                self.counts.ads.fetch_add(1, Ordering::Relaxed);
                BlockReason::Ad { filter }
            };
            Some(reason)
        } else {
            None
        }
    }

    /// Snapshot of shield counters.
    pub fn stats(&self) -> PrivacyStats {
        PrivacyStats {
            ads_blocked: self.counts.ads.load(Ordering::Relaxed),
            trackers_blocked: self.counts.trackers.load(Ordering::Relaxed),
            ..self.stats
        }
    }

    /// Cosmetic filters (element hiding) for a page — used by the JS layer
    /// to hide matched elements before first paint.
    pub fn cosmetic_filters(&self, url: &str) -> Vec<String> {
        let resources = self
            .engine
            .url_cosmetic_resources(url);
        resources.hide_selectors.into_iter().collect()
    }
}

fn looks_like_tracker(url: &str) -> bool {
    let tracker_markers = [
        "analytics", "pixel", "collect", "telemetry", "beacon", "track", "stat",
    ];
    tracker_markers.iter().any(|m| url.contains(m))
}

/// Curated default filters (EasyList/EasyPrivacy subset, ~90 lines).
/// Kept intentionally small; users can load the full lists.
pub const DEFAULT_FILTERS: &str = include_str!("default_filters.txt");

/// Shared, swappable blocker slot (the engine can hot-swap filter lists).
pub type SharedBlocker = RwLock<PrivacyBlocker>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_ad_networks() {
        let blocker = PrivacyBlocker::new();
        assert!(blocker
            .check("https://pagead2.googlesyndication.com/pagead/js/adsbygoogle.js", "news.com", RequestKind::Script)
            .is_some());
        assert!(blocker
            .check("https://google-analytics.com/analytics.js", "news.com", RequestKind::Script)
            .is_some());
    }

    #[test]
    fn allows_normal_content() {
        let blocker = PrivacyBlocker::new();
        assert!(blocker
            .check("https://example.com/styles/main.css", "example.com", RequestKind::Stylesheet)
            .is_none());
        assert!(blocker
            .check("https://cdn.jsdelivr.net/npm/vue@3/dist/vue.js", "example.com", RequestKind::Script)
            .is_none());
    }

    #[test]
    fn exception_filters_win() {
        let blocker = PrivacyBlocker::new();
        assert!(blocker
            .check("https://www.googletagmanager.com/gtm.js?id=GTM-1", "shop.com", RequestKind::Script)
            .is_none(),
            "googletagmanager has @@exception in default list");
    }

    #[test]
    fn stats_increment() {
        let blocker = PrivacyBlocker::new();
        let _ = blocker.check("https://doubleclick.net/instream/ad_status.js", "v.com", RequestKind::Script);
        let stats = blocker.stats();
        assert!(stats.ads_blocked + stats.trackers_blocked >= 1);
    }
}
