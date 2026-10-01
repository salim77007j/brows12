//! Network-layer request blocking powered by Brave's `adblock` engine.
//!
//! The embedded default list is a curated subset of EasyList/EasyPrivacy
//! covering the highest-volume trackers and ad networks so the engine is
//! protected out of the box; full lists can be loaded from disk at runtime.

use crate::PrivacyStats;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;

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

/// Thread-safe handle to the filter engine.
///
/// `adblock::Engine` is deliberately `!Send + !Sync` (interior `Rc`/`RefCell`
/// for lazy regex compilation), so the engine lives on its own dedicated
/// thread and requests are answered over a channel. `check` costs one
/// round-trip (~microseconds) and stays synchronous for callers.
#[derive(Clone)]
pub struct PrivacyBlocker {
    tx: Sender<Command>,
    stats: PrivacyStats,
    counts: Arc<BlockingCounters>,
}

enum Command {
    Check { url: String, source: String, kind: RequestKind, reply: Sender<Option<BlockReason>> },
    Cosmetic { url: String, reply: Sender<Vec<String>> },
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
        let (tx, rx) = std::sync::mpsc::channel::<Command>();
        let counts = Arc::new(BlockingCounters::default());
        let filters = filters.to_string();

        // Dedicated engine thread: owns the !Send + !Sync filter engine
        // (built here, inside the thread), and answers check/cosmetic
        // queries for every caller in the process.
        let counts_thread = Arc::clone(&counts);
        std::thread::Builder::new()
            .name("brows12-blocker".into())
            .spawn(move || {
                let mut engine = adblock::engine::Engine::new_with_list_text(&filters);
                while let Ok(cmd) = rx.recv() {
                    match cmd {
                        Command::Check { url, source, kind, reply } => {
                            let _ = reply.send(check_engine(
                                &mut engine,
                                &url,
                                &source,
                                kind,
                                &counts_thread,
                            ));
                        }
                        Command::Cosmetic { url, reply } => {
                            let selectors = engine
                                .url_cosmetic_resources(&url)
                                .hide_selectors
                                .into_iter()
                                .collect();
                            let _ = reply.send(selectors);
                        }
                    }
                }
            })
            .expect("spawn blocker thread");

        Self { tx, stats: PrivacyStats::default(), counts }
    }

    /// Should this network request be blocked?
    /// `source_hostname` is the hostname of the embedding document.
    pub fn check(
        &self,
        url: &str,
        source_hostname: &str,
        kind: RequestKind,
    ) -> Option<BlockReason> {
        let (reply_tx, reply_rx) = std::sync::mpsc::channel();
        let sent = self.tx.send(Command::Check {
            url: url.to_string(),
            source: source_hostname.to_string(),
            kind,
            reply: reply_tx,
        });
        match (sent, reply_rx.recv()) {
            (Ok(()), Ok(reason)) => reason,
            _ => None, // engine thread gone: fail open rather than hang
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
        let (reply_tx, reply_rx) = std::sync::mpsc::channel();
        let sent = self.tx.send(Command::Cosmetic { url: url.to_string(), reply: reply_tx });
        match (sent, reply_rx.recv()) {
            (Ok(()), Ok(selectors)) => selectors,
            _ => Vec::new(),
        }
    }
}

fn check_engine(
    engine: &mut adblock::engine::Engine,
    url: &str,
    source_hostname: &str,
    kind: RequestKind,
    counts: &BlockingCounters,
) -> Option<BlockReason> {
    let request =
        adblock::request::Request::new(url, source_hostname, kind.as_str(), "GET").ok()?;
    let result = engine.check_network_request(&request);
    if result.should_block() {
        let filter = result.filter.map(|f| f.raw_line).unwrap_or_default().unwrap_or_default();
        let reason = if filter.contains("privacy") || looks_like_tracker(url) {
            counts.trackers.fetch_add(1, Ordering::Relaxed);
            BlockReason::Tracker { filter }
        } else {
            counts.ads.fetch_add(1, Ordering::Relaxed);
            BlockReason::Ad { filter }
        };
        Some(reason)
    } else {
        None
    }
}

fn looks_like_tracker(url: &str) -> bool {
    let tracker_markers = ["analytics", "pixel", "collect", "telemetry", "beacon", "track", "stat"];
    tracker_markers.iter().any(|m| url.contains(m))
}

/// Curated default filters (EasyList/EasyPrivacy subset, ~90 lines).
/// Kept intentionally small; users can load the full lists.
pub const DEFAULT_FILTERS: &str = include_str!("default_filters.txt");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_ad_networks() {
        let blocker = PrivacyBlocker::new();
        assert!(blocker
            .check(
                "https://pagead2.googlesyndication.com/pagead/js/adsbygoogle.js",
                "news.com",
                RequestKind::Script
            )
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
            .check(
                "https://cdn.jsdelivr.net/npm/vue@3/dist/vue.js",
                "example.com",
                RequestKind::Script
            )
            .is_none());
    }

    #[test]
    fn exception_filters_win() {
        let blocker = PrivacyBlocker::new();
        assert!(
            blocker
                .check(
                    "https://www.googletagmanager.com/gtm.js?id=GTM-1",
                    "shop.com",
                    RequestKind::Script
                )
                .is_none(),
            "googletagmanager has @@exception in default list"
        );
    }

    #[test]
    fn stats_increment() {
        let blocker = PrivacyBlocker::new();
        let _ = blocker.check(
            "https://doubleclick.net/instream/ad_status.js",
            "v.com",
            RequestKind::Script,
        );
        let stats = blocker.stats();
        assert!(stats.ads_blocked + stats.trackers_blocked >= 1);
    }
}
