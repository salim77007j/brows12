//! Network-layer request blocking powered by Brave's `adblock` engine.
//!
//! Phase 4 Area 2 upgrade: the engine is built from the full 2026
//! EasyList + EasyPrivacy + uBlock filters lists (embedded at compile
//! time) and exposes the complete uBO modifier surface:
//!
//! * network blocking with `$important` honouring (exceptions lose),
//! * `$redirect` — serve a real replacement resource (noop.js, 1x1.gif…),
//! * `$removeparam` — strip tracking query parameters via 301 rewrite,
//! * `$csp` — surface CSP directives for document/iframe responses,
//! * cosmetic filtering (hide selectors) + uBO scriptlet injection.

use crate::PrivacyStats;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;

use crate::scriptlet_resources;

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
    /// Top-level document navigation.
    Document,
    /// Nested browsing context (iframe/frame).
    Subdocument,
    Script,
    Image,
    Stylesheet,
    Xhr,
    Font,
    Media,
    Other,
}

impl RequestKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            RequestKind::Document => "main_frame",
            RequestKind::Subdocument => "sub_frame",
            RequestKind::Script => "script",
            RequestKind::Image => "image",
            RequestKind::Stylesheet => "stylesheet",
            RequestKind::Xhr => "xmlhttprequest",
            RequestKind::Font => "font",
            RequestKind::Media => "media",
            RequestKind::Other => "other",
        }
    }
}

/// A `$redirect` replacement resource, decoded from the engine's data: URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RedirectResource {
    /// Content-Type to serve the body with.
    pub content_type: String,
    /// Raw resource body.
    pub body: Vec<u8>,
}

/// Full network-layer verdict for one request.
#[derive(Debug, Clone, Default)]
pub struct Verdict {
    /// Set when the request must not reach the network.
    pub block: Option<BlockReason>,
    /// `$redirect`: serve this instead of an empty body when blocked.
    pub redirect: Option<RedirectResource>,
    /// `$removeparam`: fetch this URL instead when allowed.
    pub rewritten_url: Option<String>,
    /// `$csp`: extra CSP directives to attach to a document/iframe response.
    pub csp_directives: Option<String>,
}

impl Verdict {
    pub fn is_blocked(&self) -> bool {
        self.block.is_some()
    }
}

/// Per-page filtering result (cosmetic + scriptlets).
#[derive(Debug, Clone, Default)]
pub struct PageScripts {
    /// CSS selectors to hide (`##…` hostname-specific + non-class/id generics).
    pub hide_selectors: Vec<String>,
    /// uBO scriptlet JavaScript to inject at document start (`##+js(…)`).
    pub injected_script: String,
    /// Generic class/id rule exceptions (`#@#…`) for this page — must be
    /// passed to [`PrivacyBlocker::hidden_class_id_selectors`].
    pub exceptions: std::collections::HashSet<String>,
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
    /// Rule count actually fed to the engine (reporting/telemetry).
    pub rules_loaded: usize,
}

enum Command {
    Check {
        url: String,
        source: String,
        kind: RequestKind,
        reply: Sender<Verdict>,
    },
    PageScripts {
        url: String,
        reply: Sender<PageScripts>,
    },
    HiddenClassId {
        classes: Vec<String>,
        ids: Vec<String>,
        exceptions: std::collections::HashSet<String>,
        reply: Sender<Vec<String>>,
    },
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
    /// Build a blocker with the embedded 2026 full lists
    /// (EasyList + EasyPrivacy + uBlock filters).
    pub fn new() -> Self {
        Self::from_filters(FULL_LISTS)
    }

    /// Build from a custom filter list text (EasyList syntax).
    /// Resources (redirect targets + scriptlets) are always installed.
    pub fn from_filters(filters: &str) -> Self {
        let rules = filters.lines().filter(counts_as_rule).count();
        let (tx, rx) = std::sync::mpsc::channel::<Command>();
        let counts = Arc::new(BlockingCounters::default());
        let filters = filters.to_string();

        // Dedicated engine thread: owns the !Send + !Sync filter engine
        // (built here, inside the thread), and answers check/page-scripts
        // queries for every caller in the process.
        let counts_thread = Arc::clone(&counts);
        std::thread::Builder::new()
            .name("brows12-blocker".into())
            .spawn(move || {
                let started = std::time::Instant::now();
                let mut engine = adblock::engine::Engine::new_with_list_text(&filters);
                // Redirect targets + scriptlet templates must be installed
                // before any request for `$redirect`/`##+js()` to resolve.
                engine.use_resources(scriptlet_resources::all());
                // $removeparam matching runs locally (upstream 0.13.3 parses
                // but never matches these rules — see removeparam.rs).
                let removeparam_rules = crate::removeparam::default_rules();
                let build_ms = started.elapsed().as_millis();
                eprintln!(
                    "brows12-privacy: filter engine ready ({} rules, {} ms, {} resources, {} removeparam rules)",
                    rules,
                    build_ms,
                    scriptlet_resources::all().len(),
                    removeparam_rules.len()
                );
                while let Ok(cmd) = rx.recv() {
                    match cmd {
                        Command::Check { url, source, kind, reply } => {
                            let _ = reply.send(check_engine(
                                &engine,
                                &removeparam_rules,
                                &url,
                                &source,
                                kind,
                                &counts_thread,
                            ));
                        }
                        Command::PageScripts { url, reply } => {
                            let _ = reply.send(page_scripts(&engine, &url));
                        }
                        Command::HiddenClassId { classes, ids, exceptions, reply } => {
                            let selectors = engine
                                .hidden_class_id_selectors(classes, ids, &exceptions);
                            let _ = reply.send(selectors);
                        }
                    }
                }
            })
            .expect("spawn blocker thread");

        Self { tx, stats: PrivacyStats::default(), counts, rules_loaded: rules }
    }

    /// Full verdict for a network request.
    /// `source_hostname` is the hostname of the embedding document.
    pub fn check(&self, url: &str, source_hostname: &str, kind: RequestKind) -> Verdict {
        let (reply_tx, reply_rx) = std::sync::mpsc::channel();
        let sent = self.tx.send(Command::Check {
            url: url.to_string(),
            source: source_hostname.to_string(),
            kind,
            reply: reply_tx,
        });
        match (sent, reply_rx.recv()) {
            (Ok(()), Ok(verdict)) => verdict,
            _ => Verdict::default(), // engine thread gone: fail open rather than hang
        }
    }

    /// Convenience: should this network request be blocked?
    pub fn should_block(&self, url: &str, source_hostname: &str, kind: RequestKind) -> bool {
        self.check(url, source_hostname, kind).is_blocked()
    }

    /// Snapshot of shield counters.
    pub fn stats(&self) -> PrivacyStats {
        PrivacyStats {
            ads_blocked: self.counts.ads.load(Ordering::Relaxed),
            trackers_blocked: self.counts.trackers.load(Ordering::Relaxed),
            ..self.stats
        }
    }

    /// Cosmetic + scriptlet filters for a page — installed as user content
    /// before the document loads.
    pub fn page_scripts(&self, url: &str) -> PageScripts {
        let (reply_tx, reply_rx) = std::sync::mpsc::channel();
        let sent = self.tx.send(Command::PageScripts { url: url.to_string(), reply: reply_tx });
        match (sent, reply_rx.recv()) {
            (Ok(()), Ok(scripts)) => scripts,
            _ => PageScripts::default(),
        }
    }

    /// Cosmetic filters (element hiding) for a page.
    pub fn cosmetic_filters(&self, url: &str) -> Vec<String> {
        self.page_scripts(url).hide_selectors
    }

    /// Generic class/id rule exceptions for a page (`#@#…`), used together
    /// with [`Self::hidden_class_id_selectors`].
    pub fn cosmetic_exceptions(&self, url: &str) -> std::collections::HashSet<String> {
        self.page_scripts(url).exceptions
    }

    /// Generic class/id-based cosmetic rules matching the page's DOM.
    ///
    /// uBO-compatible two-phase protocol: `url_cosmetic_resources` returns
    /// hostname-specific selectors + `exceptions`; the page's actual class
    /// and id attributes are then matched here. Simple class/id generics
    /// (the bulk of EasyList `##…` rules) live in this phase, not in
    /// `hide_selectors`.
    pub fn hidden_class_id_selectors(
        &self,
        classes: Vec<String>,
        ids: Vec<String>,
        exceptions: std::collections::HashSet<String>,
    ) -> Vec<String> {
        let (reply_tx, reply_rx) = std::sync::mpsc::channel();
        let sent =
            self.tx.send(Command::HiddenClassId { classes, ids, exceptions, reply: reply_tx });
        match (sent, reply_rx.recv()) {
            (Ok(()), Ok(selectors)) => selectors,
            _ => Vec::new(),
        }
    }
}

fn counts_as_rule(line: &&str) -> bool {
    let t = line.trim_start();
    if t.is_empty() {
        return false;
    }
    // Metadata comments are not rules (but exception/comment-marked filters
    // still count via their first char below).
    !matches!(t.as_bytes().first(), Some(b'!') | Some(b'['))
}

fn check_engine(
    engine: &adblock::engine::Engine,
    removeparam_rules: &[crate::removeparam::RemoveparamRule],
    url: &str,
    source_hostname: &str,
    kind: RequestKind,
    counts: &BlockingCounters,
) -> Verdict {
    let Ok(request) = adblock::request::Request::new(url, source_hostname, kind.as_str(), "GET")
    else {
        return Verdict::default();
    };

    let result = engine.check_network_request(&request);

    // $csp only applies to document/subdocument responses.
    let csp_directives = matches!(kind, RequestKind::Document | RequestKind::Subdocument)
        .then(|| engine.get_csp_directives(&request))
        .flatten();

    if result.should_block() {
        let filter = result.filter.map(|f| f.raw_line).unwrap_or_default().unwrap_or_default();
        let reason = if filter.contains("privacy") || looks_like_tracker(url) {
            counts.trackers.fetch_add(1, Ordering::Relaxed);
            BlockReason::Tracker { filter }
        } else {
            counts.ads.fetch_add(1, Ordering::Relaxed);
            BlockReason::Ad { filter }
        };
        let redirect = result
            .redirect
            .as_deref()
            .and_then(decode_data_url)
            .map(|(ct, body)| RedirectResource { content_type: ct, body });
        return Verdict { block: Some(reason), redirect, ..Default::default() };
    }

    // $removeparam: upstream 0.13.3 parses these rules but never matches
    // them; run our own matcher over the brows12 extras rules and prefer
    // its result (the crate's rewritten_url is kept as a fallback for
    // newer engine versions).
    let rewritten_url = crate::removeparam::strip_params(
        removeparam_rules,
        url,
        &request.hostname,
        source_hostname,
    )
    .or(result.rewritten_url);

    Verdict { block: None, redirect: None, rewritten_url, csp_directives }
}

/// Cosmetic + scriptlet set for one page URL.
fn page_scripts(engine: &adblock::engine::Engine, url: &str) -> PageScripts {
    let resources = engine.url_cosmetic_resources(url);
    PageScripts {
        hide_selectors: resources.hide_selectors.into_iter().collect(),
        injected_script: resources.injected_script,
        exceptions: resources.exceptions,
    }
}

fn looks_like_tracker(url: &str) -> bool {
    let tracker_markers = ["analytics", "pixel", "collect", "telemetry", "beacon", "track", "stat"];
    tracker_markers.iter().any(|m| url.contains(m))
}

/// Decode `data:<mime>;base64,<payload>` (what the engine returns for
/// `$redirect` resources) into a content type + body.
fn decode_data_url(data_url: &str) -> Option<(String, Vec<u8>)> {
    use base64::Engine as _;
    let rest = data_url.strip_prefix("data:")?;
    let (meta, payload_b64) = rest.split_once(',')?;
    let content_type = meta
        .split(';')
        .next()
        .filter(|m| !m.is_empty())
        .unwrap_or("application/octet-stream")
        .to_string();
    let body = base64::engine::general_purpose::STANDARD.decode(payload_b64).ok()?;
    Some((content_type, body))
}

/// Embedded 2026 filter lists (refreshed 2026-10-04 from easylist.to and
/// uBlockOrigin/uAssets). ~4.3 MB, ~152k rules.
pub const EASYLIST: &str = include_str!("../lists/easylist.txt");
pub const EASYPRIVACY: &str = include_str!("../lists/easyprivacy.txt");
pub const UBO_FILTERS: &str = include_str!("../lists/ubofilters.txt");
pub const UBO_PRIVACY: &str = include_str!("../lists/uboprivacy.txt");

/// brows12 policy layer on top of the public lists: universal tracking
/// query-parameter stripping (`$removeparam`) that uBO leaves out of its
/// defaults but the strongest 2026 shields apply. See the file header.
pub const BROWS12_EXTRAS: &str = include_str!("../lists/brows12_extras.txt");

/// The default active set: ads + privacy trackers + uBO modifiers +
/// uBO privacy rules + brows12 extras.
pub const FULL_LISTS: &str = concat!(
    include_str!("../lists/easylist.txt"),
    "\n",
    include_str!("../lists/easyprivacy.txt"),
    "\n",
    include_str!("../lists/ubofilters.txt"),
    "\n",
    include_str!("../lists/uboprivacy.txt"),
    "\n",
    include_str!("../lists/brows12_extras.txt"),
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_ad_networks_full_lists() {
        let blocker = PrivacyBlocker::new();
        assert!(blocker.rules_loaded > 50_000, "full 2026 lists must be loaded");
        assert!(blocker
            .check(
                "https://pagead2.googlesyndication.com/pagead/js/adsbygoogle.js",
                "news.com",
                RequestKind::Script
            )
            .is_blocked());
        assert!(blocker
            .check("https://www.google-analytics.com/analytics.js", "news.com", RequestKind::Script)
            .is_blocked());
    }

    #[test]
    fn allows_normal_content() {
        let blocker = PrivacyBlocker::new();
        assert!(!blocker
            .check("https://example.com/styles/main.css", "example.com", RequestKind::Stylesheet)
            .is_blocked());
        assert!(!blocker
            .check(
                "https://cdn.jsdelivr.net/npm/vue@3/dist/vue.js",
                "example.com",
                RequestKind::Script
            )
            .is_blocked());
    }

    #[test]
    fn important_beats_exceptions() {
        // $important must block even when a matching exception exists.
        let blocker = PrivacyBlocker::from_filters(
            "||important.test/blocked.js$script,important\n@@||important.test/blocked.js$script\n",
        );
        let v = blocker.check("https://important.test/blocked.js", "news.com", RequestKind::Script);
        assert!(v.is_blocked(), "$important must win over the exception");
    }

    #[test]
    fn redirect_serves_replacement_resource() {
        let blocker =
            PrivacyBlocker::from_filters("||redirect.test/track.js$script,redirect=noop.js\n");
        let v = blocker.check("https://redirect.test/track.js", "news.com", RequestKind::Script);
        assert!(v.is_blocked());
        let r = v.redirect.expect("noop.js redirect resource must resolve");
        assert_eq!(r.content_type, "application/javascript");
        assert!(!r.body.is_empty());
        let body = String::from_utf8_lossy(&r.body);
        assert!(body.contains("function ()"), "noop.js body must be JS: {body}");
    }

    #[test]
    fn removeparam_rewrites_url() {
        let blocker =
            PrivacyBlocker::from_filters("*$removeparam=utm_source\n*$removeparam=fbclid\n");
        let v = blocker.check(
            "https://news.test/article?utm_source=spam&id=7",
            "news.test",
            RequestKind::Document,
        );
        assert!(!v.is_blocked());
        let rewritten = v.rewritten_url.expect("utm_source must be stripped via $removeparam");
        assert!(rewritten.contains("id=7"));
        assert!(!rewritten.contains("utm_source"));
    }

    #[test]
    fn csp_directives_surface_for_documents() {
        let blocker = PrivacyBlocker::from_filters("||csp.test^$csp=script-src 'none'\n");
        let v =
            blocker.check("https://ads.csp.test/frame.html", "news.com", RequestKind::Subdocument);
        assert!(
            v.csp_directives.as_deref().map(|d| d.contains("script-src 'none'")).unwrap_or(false),
            "$csp directive must surface for subdocuments"
        );
        // Not for scripts.
        let v2 = blocker.check("https://ads.csp.test/lib.js", "news.com", RequestKind::Script);
        assert!(v2.csp_directives.is_none());
    }

    #[test]
    fn cosmetic_and_scriptlet_page_scripts() {
        let blocker =
            PrivacyBlocker::from_filters("news.test##.ad-banner\nnews.test##+js(nowebrtc)\n");
        let scripts = blocker.page_scripts("https://news.test/frontpage?utm_source=x");
        assert!(scripts.hide_selectors.iter().any(|s| s.contains(".ad-banner")));
        assert!(
            scripts.injected_script.contains("RTCPeerConnection"),
            "scriptlet must be rendered into injected_script"
        );
    }

    #[test]
    fn subdocument_kind_matches_sub_frame_rules() {
        // A rule targeting subdocuments specifically.
        let blocker = PrivacyBlocker::from_filters("||frames.test/embed.html$subdocument\n");
        let v =
            blocker.check("https://frames.test/embed.html", "news.com", RequestKind::Subdocument);
        assert!(v.is_blocked());
        // Same URL as a top-level document is NOT blocked by this rule.
        let v2 =
            blocker.check("https://frames.test/embed.html", "frames.test", RequestKind::Document);
        assert!(!v2.is_blocked());
    }

    #[test]
    fn stats_increment() {
        let blocker = PrivacyBlocker::new();
        let _ = blocker.check(
            "https://connect.facebook.net/en_US/fbevents.js",
            "v.com",
            RequestKind::Script,
        );
        let stats = blocker.stats();
        assert!(stats.ads_blocked + stats.trackers_blocked >= 1);
    }

    #[test]
    fn full_lists_honour_legit_exceptions() {
        // 2026 EasyList/EasyPrivacy deliberately allow-list YouTube's
        // ad_status.js probe (it is used by legit embeds); the exception
        // mechanism must be honoured by the engine.
        let blocker = PrivacyBlocker::new();
        let v = blocker.check(
            "https://doubleclick.net/instream/ad_status.js",
            "youtube.com",
            RequestKind::Script,
        );
        assert!(
            !v.is_blocked(),
            "2026 lists exception for doubleclick/instream/ad_status.js must win"
        );
    }
}
