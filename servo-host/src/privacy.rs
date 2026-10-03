//! brows12 privacy layer, integrated at the Servo embedder hooks.
//!
//! Network blocking runs in `WebViewDelegate::load_web_resource`: every
//! resource request Servo is about to make (post-CORS, with destination
//! metadata) is checked against the adblock/tracker matcher; blocked
//! requests are answered with an empty 200 so page JS sees a settled
//! resource rather than a network error.
//!
//! Cosmetic filtering runs at navigation time: the matcher's CSS hide
//! selectors for the target URL are injected as a user stylesheet through
//! Servo's `UserContentManager` before the document paints.
//!
//! All of this reuses the v1 `brows12-privacy` matcher unchanged — the
//! lists, the CHIPS-aware policy hooks and the counters survive the pivot.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;

use brows12_privacy::blocker::{BlockReason, RequestKind};
use brows12_privacy::PrivacyBlocker;
use std::rc::Rc;
use std::sync::Arc;
use content_security_policy::Destination;
use servo::UserContentManager;
use embedder_traits::user_contents::UserStyleSheet;

pub struct PrivacyHost {
    pub blocker: PrivacyBlocker,
    pub enabled: AtomicBool,
    pub cosmetic_enabled: AtomicBool,
    pub ads_blocked: AtomicU64,
    pub trackers_blocked: AtomicU64,
    /// URLs blocked this session (capped for the report).
    pub blocked_log: Mutex<Vec<String>>,
    ucm: Mutex<Option<Rc<UserContentManager>>>,
    cosmetic_stylesheet: Mutex<Option<Rc<UserStyleSheet>>>,
}

impl PrivacyHost {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            blocker: PrivacyBlocker::new(),
            enabled: AtomicBool::new(true),
            cosmetic_enabled: AtomicBool::new(true),
            ads_blocked: AtomicU64::new(0),
            trackers_blocked: AtomicU64::new(0),
            blocked_log: Mutex::new(Vec::new()),
            ucm: Mutex::new(None),
            cosmetic_stylesheet: Mutex::new(None),
        })
    }

    pub fn set_user_content_manager(&self, ucm: Rc<UserContentManager>) {
        *self.ucm.lock().unwrap() = Some(ucm);
    }

    pub fn set_enabled(&self, on: bool) {
        self.enabled.store(on, Ordering::Relaxed);
    }

    /// Decides whether a resource request should be blocked.
    pub fn should_block(
        &self,
        url: &str,
        source_hostname: &str,
        kind: RequestKind,
    ) -> Option<BlockReason> {
        if !self.enabled.load(Ordering::Relaxed) {
            return None;
        }
        self.blocker.check(url, source_hostname, kind)
    }

    pub fn record_block(&self, reason: BlockReason, url: &str) {
        match reason {
            BlockReason::Ad { .. } => {
                self.ads_blocked.fetch_add(1, Ordering::Relaxed);
            },
            BlockReason::Tracker { .. } => {
                self.trackers_blocked.fetch_add(1, Ordering::Relaxed);
            },
        }
        let mut log = self.blocked_log.lock().unwrap();
        if log.len() < 512 {
            log.push(url.to_string());
        }
    }

    /// Replaces the cosmetic-filter user stylesheet for the current page.
    /// UCM changes apply from the next document load, so this is called
    /// from `request_navigation` (before the document is fetched).
    pub fn set_cosmetic_filters(&self, page_url: &str) {
        if !self.cosmetic_enabled.load(Ordering::Relaxed) {
            return;
        }
        let Some(ucm) = self.ucm.lock().unwrap().clone() else {
            return;
        };
        let mut slot = self.cosmetic_stylesheet.lock().unwrap();
        if let Some(old) = slot.take() {
            ucm.remove_stylesheet(old);
        }
        let selectors = self.blocker.cosmetic_filters(page_url);
        if selectors.is_empty() {
            return;
        }
        // adblock's hide selectors are CSS selector lists already.
        let css = format!(
            "{} {{ display: none !important; }}\n",
            selectors.join(",\n")
        );
        let url: url::Url = format!("brows12://privacy/cosmetic?{}", urlencoding_lite(page_url))
            .parse()
            .unwrap_or_else(|_| url::Url::parse("brows12://privacy/cosmetic").unwrap());
        let sheet = Rc::new(UserStyleSheet::new(css, url));
        ucm.add_stylesheet(sheet.clone());
        *slot = Some(sheet);
    }

    pub fn blocked_summary(&self) -> (u64, u64, Vec<String>) {
        (
            self.ads_blocked.load(Ordering::Relaxed),
            self.trackers_blocked.load(Ordering::Relaxed),
            self.blocked_log.lock().unwrap().clone(),
        )
    }
}

/// Maps Servo's CSP request destination onto our privacy RequestKind.
pub fn destination_to_kind(destination: &Destination) -> RequestKind {
    match destination {
        Destination::Document | Destination::Frame | Destination::IFrame | Destination::Embed => {
            RequestKind::Document
        },
        Destination::Script
        | Destination::AudioWorklet
        | Destination::PaintWorklet
        | Destination::Worker
        | Destination::SharedWorker
        | Destination::ServiceWorker => RequestKind::Script,
        Destination::Image | Destination::Audio | Destination::Video | Destination::Track => {
            RequestKind::Image
        },
        Destination::Style | Destination::Xslt => RequestKind::Stylesheet,
        Destination::Font => RequestKind::Font,
        Destination::Json | Destination::Manifest | Destination::Report => RequestKind::Xhr,
        Destination::None | Destination::Object | Destination::WebIdentity => RequestKind::Other,
    }
}

fn urlencoding_lite(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            },
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}
