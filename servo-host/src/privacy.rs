//! brows12 privacy layer, integrated at the Servo embedder hooks.
//!
//! Network blocking runs in `WebViewDelegate::load_web_resource`: every
//! resource request Servo is about to make (post-CORS, with destination
//! metadata) receives a full `Verdict` from the adblock matcher:
//!
//! * blocked + `$redirect` → serve the real replacement resource body,
//! * blocked otherwise → empty 200 so page JS sees a settled resource,
//! * allowed + `$removeparam` → serve a 301 to the stripped URL,
//! * allowed + `$csp` (documents) → recorded for meta-CSP injection.
//!
//! Cosmetic filtering and uBO scriptlet injection run at navigation time:
//! the matcher's hide selectors become a user stylesheet and its scriptlet
//! code a user script, both installed through Servo's `UserContentManager`
//! before the document paints.

use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use brows12_privacy::blocker::{BlockReason, RequestKind, Verdict};
use brows12_privacy::PrivacyBlocker;
use content_security_policy::Destination;
use embedder_traits::user_contents::UserStyleSheet;
use servo::UserContentManager;

#[allow(clippy::arc_with_non_send_sync)]
pub struct PrivacyHost {
    pub blocker: PrivacyBlocker,
    pub enabled: AtomicBool,
    pub cosmetic_enabled: AtomicBool,
    pub ads_blocked: AtomicU64,
    pub trackers_blocked: AtomicU64,
    /// Requests answered with a `$redirect` replacement resource.
    pub redirects_served: AtomicU64,
    /// Requests rewritten via `$removeparam`.
    pub params_stripped: AtomicU64,
    /// `$csp` directive sets recorded for document responses.
    pub csp_injections: AtomicU64,
    /// Pages that received uBO scriptlet code.
    pub scriptlets_injected: AtomicU64,
    /// URLs blocked this session (capped for the report).
    pub blocked_log: Mutex<Vec<String>>,
    /// URLs with `$csp` directives for the current page (capped).
    pub csp_log: Mutex<Vec<String>>,
    /// Pages that received phase-2 (class/id) cosmetic hiding.
    pub cosmetic_pages_filtered: AtomicU64,
    ucm: Mutex<Option<Rc<UserContentManager>>>,
    cosmetic_stylesheet: Mutex<Option<Rc<UserStyleSheet>>>,
    scriptlet_script: Mutex<Option<Rc<servo::user_contents::UserScript>>>,
    csp_script: Mutex<Option<Rc<servo::user_contents::UserScript>>>,
}

impl PrivacyHost {
    #[allow(clippy::arc_with_non_send_sync)]
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            blocker: PrivacyBlocker::new(),
            enabled: AtomicBool::new(true),
            cosmetic_enabled: AtomicBool::new(true),
            ads_blocked: AtomicU64::new(0),
            trackers_blocked: AtomicU64::new(0),
            redirects_served: AtomicU64::new(0),
            params_stripped: AtomicU64::new(0),
            csp_injections: AtomicU64::new(0),
            scriptlets_injected: AtomicU64::new(0),
            blocked_log: Mutex::new(Vec::new()),
            csp_log: Mutex::new(Vec::new()),
            cosmetic_pages_filtered: AtomicU64::new(0),
            ucm: Mutex::new(None),
            cosmetic_stylesheet: Mutex::new(None),
            scriptlet_script: Mutex::new(None),
            csp_script: Mutex::new(None),
        })
    }

    pub fn set_user_content_manager(&self, ucm: Rc<UserContentManager>) {
        *self.ucm.lock().unwrap() = Some(ucm);
    }

    pub fn set_enabled(&self, on: bool) {
        self.enabled.store(on, Ordering::Relaxed);
    }

    /// Full network verdict for a request (blocking is gated by `enabled`,
    /// rewrite/csp info is always computed).
    pub fn verdict(&self, url: &str, source_hostname: &str, kind: RequestKind) -> Verdict {
        let mut v = self.blocker.check(url, source_hostname, kind);
        if !self.enabled.load(Ordering::Relaxed) {
            v.block = None;
        }
        v
    }

    pub fn record_block(&self, reason: BlockReason, url: &str) {
        match reason {
            BlockReason::Ad { .. } => {
                self.ads_blocked.fetch_add(1, Ordering::Relaxed);
            }
            BlockReason::Tracker { .. } => {
                self.trackers_blocked.fetch_add(1, Ordering::Relaxed);
            }
        }
        let mut log = self.blocked_log.lock().unwrap();
        if log.len() < 512 {
            log.push(url.to_string());
        }
    }

    /// Installs per-page user content for `page_url`: cosmetic-hide
    /// stylesheet, uBO scriptlet script, and (when the page's own verdict
    /// carries `$csp` directives) a meta-CSP injection script.
    /// UCM changes apply from the next document load, so this is called
    /// from `request_navigation` (before the document is fetched).
    pub fn set_page_filtering(&self, page_url: &str) {
        let Some(ucm) = self.ucm.lock().unwrap().clone() else {
            return;
        };

        // 1. Cosmetic hide selectors.
        if self.cosmetic_enabled.load(Ordering::Relaxed) {
            let mut slot = self.cosmetic_stylesheet.lock().unwrap();
            if let Some(old) = slot.take() {
                ucm.remove_stylesheet(old);
            }
            let scripts = self.blocker.page_scripts(page_url);
            if !scripts.hide_selectors.is_empty() {
                // adblock's hide selectors are CSS selector lists already.
                let css = format!(
                    "{} {{ display: none !important; }}\n",
                    scripts.hide_selectors.join(",\n")
                );
                let url: url::Url =
                    format!("brows12://privacy/cosmetic?{}", urlencoding_lite(page_url))
                        .parse()
                        .unwrap_or_else(|_| url::Url::parse("brows12://privacy/cosmetic").unwrap());
                let sheet = Rc::new(UserStyleSheet::new(css, url));
                ucm.add_stylesheet(sheet.clone());
                *slot = Some(sheet);
            }

            // 2. uBO scriptlet injection.
            let mut slot = self.scriptlet_script.lock().unwrap();
            if let Some(old) = slot.take() {
                ucm.remove_script(old);
            }
            if !scripts.injected_script.trim().is_empty() {
                let script =
                    Rc::new(servo::user_contents::UserScript::new(scripts.injected_script, None));
                ucm.add_script(script.clone());
                self.scriptlets_injected.fetch_add(1, Ordering::Relaxed);
                *slot = Some(script);
            }
        }

        // 3. $csp for the top-level document itself.
        let mut slot = self.csp_script.lock().unwrap();
        if let Some(old) = slot.take() {
            ucm.remove_script(old);
        }
        let doc_verdict =
            self.blocker.check(page_url, &hostname_of(page_url), RequestKind::Document);
        if let Some(directives) = doc_verdict.csp_directives {
            self.csp_injections.fetch_add(1, Ordering::Relaxed);
            let mut log = self.csp_log.lock().unwrap();
            if log.len() < 128 {
                log.push(format!("{page_url} :: {directives}"));
            }
            let script =
                Rc::new(servo::user_contents::UserScript::new(csp_meta_script(&directives), None));
            ucm.add_script(script.clone());
            *slot = Some(script);
        }
    }

    pub fn blocked_summary(&self) -> (u64, u64, Vec<String>) {
        (
            self.ads_blocked.load(Ordering::Relaxed),
            self.trackers_blocked.load(Ordering::Relaxed),
            self.blocked_log.lock().unwrap().clone(),
        )
    }

    /// Records `$csp` directives surfaced for a pass-through iframe/subframe
    /// response (we cannot attach response headers on the pass-through path;
    /// the recording feeds the report and future engine integration).
    pub fn record_csp(&self, url: &str, directives: &str) {
        self.csp_injections.fetch_add(1, Ordering::Relaxed);
        let mut log = self.csp_log.lock().unwrap();
        if log.len() < 128 {
            log.push(format!("{url} :: {directives}"));
        }
    }

    /// Phase-2 cosmetic matching from a DOM-info JSON payload collected by
    /// `DOM_INFO_JS`. Returns the CSS selectors to hide.
    pub fn hidden_selectors_for_page(&self, dom_info_json: &str) -> Vec<String> {
        let Ok(info) = serde_json::from_str::<DomInfo>(dom_info_json) else {
            return Vec::new();
        };
        let exceptions = self.blocker.cosmetic_exceptions(&info.url);
        self.blocker.hidden_class_id_selectors(info.classes, info.ids, exceptions)
    }
}

#[derive(serde::Deserialize)]
struct DomInfo {
    url: String,
    #[serde(default)]
    classes: Vec<String>,
    #[serde(default)]
    ids: Vec<String>,
}

/// Collects the document's class/id attributes for phase-2 cosmetic
/// matching (runs in the page, result parsed in Rust).
pub const DOM_INFO_JS: &str = r#"(function () {
  try {
    var classes = {}, ids = {};
    var els = document.querySelectorAll('*');
    var count = 0, MAX = 4000;
    for (var i = 0; i < els.length && count < MAX; i++) {
      var el = els[i];
      if (el.id) { ids[el.id] = 1; count++; }
      var cl = el.classList;
      for (var j = 0; j < cl.length && count < MAX; j++) {
        if (!classes[cl[j]]) { classes[cl[j]] = 1; count++; }
      }
    }
    return JSON.stringify({
      url: String(location.href),
      classes: Object.keys(classes),
      ids: Object.keys(ids)
    });
  } catch (e) { return ''; }
})()"#;

/// Builds a JS snippet that installs a hide stylesheet for `selectors`.
pub fn hide_stylesheet_js(selectors: &str) -> String {
    // Escape for a JS single-quoted string; selectors never contain NUL.
    let escaped = selectors.replace('\\', "\\\\").replace('\'', "\\'").replace('\n', "\\n");
    format!(
        r#"(function () {{
  try {{
    var s = document.createElement('style');
    s.setAttribute('data-brows12', 'cosmetic-phase2');
    s.textContent = '{escaped} {{ display: none !important; }}';
    (document.head || document.documentElement).appendChild(s);
  }} catch (e) {{}}
}})();"#
    )
}

/// JS that installs a meta CSP tag as soon as the document allows it.
/// Defense-in-depth for `$csp` filters: applies from insertion onward
/// (dynamic content, late subresources). Limitations documented in the
/// Area 2 report.
fn csp_meta_script(directives: &str) -> String {
    // Escape for embedding in a JS string literal.
    let escaped = directives.replace('\\', "\\\\").replace('"', "\\\"");
    format!(
        r#"(function () {{
  var directives = "{escaped}";
  function install() {{
    if (document.querySelector('meta[http-equiv="Content-Security-Policy"]')) return;
    var meta = document.createElement('meta');
    meta.setAttribute('http-equiv', 'Content-Security-Policy');
    meta.setAttribute('content', directives);
    (document.head || document.documentElement).appendChild(meta);
  }}
  if (document.head || document.documentElement) {{ install(); }}
  else {{ document.addEventListener('DOMContentLoaded', install); }}
}})();"#
    )
}

fn hostname_of(url: &str) -> String {
    url::Url::parse(url).ok().and_then(|u| u.host_str().map(|s| s.to_string())).unwrap_or_default()
}

/// Maps Servo's CSP request destination onto our privacy RequestKind.
pub fn destination_to_kind(destination: &Destination) -> RequestKind {
    match destination {
        Destination::Document | Destination::Embed => RequestKind::Document,
        Destination::Frame | Destination::IFrame => RequestKind::Subdocument,
        Destination::Script
        | Destination::AudioWorklet
        | Destination::PaintWorklet
        | Destination::Worker
        | Destination::SharedWorker
        | Destination::ServiceWorker => RequestKind::Script,
        Destination::Audio | Destination::Video | Destination::Track => RequestKind::Media,
        Destination::Image => RequestKind::Image,
        Destination::Font => RequestKind::Font,
        Destination::Style | Destination::Xslt => RequestKind::Stylesheet,
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
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}
