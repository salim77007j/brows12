//! The brows12 WebView delegate: observes load progress, frames, console
//! output, intercepts web resources for the privacy layer (Phase 1.5),
//! and prepares cosmetic filters at navigation time.

use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use http::header;
use http::{HeaderMap, StatusCode};
use servo::{
    ConsoleLogLevel, CreateNewWebViewRequest, LoadStatus, MediaSessionEvent,
    MediaSessionPlaybackState, NavigationRequest, WebResourceLoad, WebResourceResponse, WebView,
    WebViewDelegate,
};
use url::Url;

use brows12_privacy::blocker::RequestKind;

use crate::privacy::{destination_to_kind, PrivacyHost};

#[derive(Default)]
pub struct DelegateState {
    url: Option<String>,
    title: Option<String>,
    console: Vec<String>,
    crash: Option<String>,
}

/// Shared observable state for the host runner. The runner polls this to
/// decide when a page has settled and a capture may be taken.
#[derive(Default)]
pub struct HostState {
    pub load_status: Mutex<Option<LoadStatus>>,
    pub frames: AtomicU64,
    pub animating: AtomicBool,
    pub complete_at: Mutex<Option<Instant>>,
    pub state: Mutex<DelegateState>,
    /// Phase 3.9: resources requested by the current document (both
    /// blocked and passed). Proxy for page weight — feeds the governor's
    /// heavy-tab policy. Reset on each navigation.
    pub page_requests: AtomicU64,
    /// Phase 4 Area 2: JS snippets queued from inside evaluation callbacks.
    /// `evaluate_javascript` cannot be called re-entrantly (the evaluator
    /// `RefCell` is still borrowed while a result callback runs), so
    /// follow-up evaluations are drained by the host loop each iteration.
    pub pending_js: Mutex<Vec<std::string::String>>,
    /// Phase 4 Area 2.3: recent cross-domain navigation hops (time,
    /// registrable domain) — feeds the interstitial redirect-chain guard.
    pub nav_hops: Mutex<Vec<(std::time::Instant, String)>>,
    /// Phase 4.3.4: unix-millis timestamp of the last observable page
    /// activity (a new frame — rAF/timer-driven repaint — or a resource
    /// request). A *background* tab that keeps touching this is running
    /// idle timers/animations; the budget governor hibernates it on the
    /// heavy-tab schedule instead of the light one. 0 = never active.
    pub last_activity_ms: AtomicU64,
    /// v2.1 Phase 1.2: unix-millis timestamp of the FIRST observable
    /// activity of the current document (0 = none yet). Anchors the
    /// embedder completion criterion's hard cap.
    pub first_activity_ms: AtomicU64,
    /// v2.1 Phase 1.2: when the embedder completion criterion first
    /// fired for the current document (latched by the runner loop via
    /// `settled_after_completion`). The settle wait counts from here.
    pub embedder_complete_at: Mutex<Option<Instant>>,
    /// Phase 4.4.2: form-state snapshot (JSON array, `SERIALIZE_FORMS_JS`)
    /// captured when the tab goes to the background. Read at hibernation
    /// time so a discarded tab can restore its form inputs on reload.
    pub form_snapshot: Mutex<Option<String>>,
    /// Phase 4.4.5: page-content snippet (meta description / first
    /// heading) captured at load completion, feeding the tab-search
    /// index.
    pub page_snippet: Mutex<Option<String>>,
    /// Phase 4.4.7: the page is currently PLAYING media (from the
    /// engine's media-session playback-state events). A playing tab is
    /// exempt from idle suspension. Reset on navigation.
    pub media_playing: AtomicBool,
}

/// uBO-style form-state capture, run when a tab goes to the background:
/// one JSON array entry per input/textarea/select, by document order.
/// Stored in `HostState::form_snapshot` by the evaluation callback.
pub const SERIALIZE_FORMS_JS: &str = r#"
(function(){
  var els = document.querySelectorAll('input,textarea,select');
  var out = [];
  for (var i = 0; i < els.length; i++) {
    var e = els[i];
    if (e.type === 'checkbox' || e.type === 'radio') { out.push([e.type, e.checked ? 1 : 0]); }
    else if (e.tagName === 'SELECT') { out.push(['select', e.selectedIndex]); }
    else { out.push([e.type || 'text', e.value]); }
  }
  return JSON.stringify(out);
})()
"#;

/// Phase 4.4.5: page-content snippet for the tab-search index — meta
/// description and/or the first heading, capped at 240 chars.
pub const SNIPPET_JS: &str = r#"
(function(){
  var m = document.querySelector('meta[name="description"], meta[property="og:description"]');
  var h = document.querySelector('h1');
  var parts = [];
  if (m && m.content) { parts.push(m.content.trim()); }
  if (h && h.textContent) { parts.push(h.textContent.trim().slice(0, 120)); }
  return parts.join(' | ').slice(0, 240);
})()
"#;

/// Build the form-restore snippet for a captured JSON snapshot.
/// `window.scrollTo` and the browser clamp the scroll target to the
/// document, so a stale estimate can never overscroll.
pub fn restore_state_js(scroll_y: f32, form_json: Option<&str>) -> String {
    let forms = match form_json {
        Some(json) if !json.is_empty() => format!(
            "var d = {json};
             var els = document.querySelectorAll('input,textarea,select');
             for (var i = 0; i < d.length && i < els.length; i++) {{
               var e = els[i], v = d[i]; if (!v) continue;
               if (v[0] === 'checkbox' || v[0] === 'radio') {{ e.checked = !!v[1]; }}
               else if (v[0] === 'select') {{ e.selectedIndex = v[1] | 0; }}
               else {{ e.value = v[1]; }}
             }}"
        ),
        _ => String::new(),
    };
    format!(
        "(function(){{ window.scrollTo(0, {scroll_y}); {forms} }})()",
        scroll_y = scroll_y.max(0.0),
        forms = forms
    )
}

/// v2.1 Phase 1.2: env-tunable quiet period for the embedder completion
/// criterion (`BROWS12_COMPLETE_QUIET_MS`, default 2000).
pub fn env_complete_quiet_ms() -> u64 {
    std::env::var("BROWS12_COMPLETE_QUIET_MS")
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(2_000)
}

/// v2.1 Phase 1.2: env-tunable hard cap since first activity for the
/// embedder completion criterion (`BROWS12_COMPLETE_MAX_WAIT_MS`,
/// default 15000).
pub fn env_complete_max_wait_ms() -> u64 {
    std::env::var("BROWS12_COMPLETE_MAX_WAIT_MS")
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(15_000)
}

fn unix_now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

impl HostState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Drains queued follow-up JS snippets (called by host loops outside
    /// evaluation-callback context).
    pub fn drain_pending_js(&self) -> Vec<String> {
        std::mem::take(&mut *self.pending_js.lock().unwrap())
    }

    pub fn frame_count(&self) -> u64 {
        self.frames.load(Ordering::Relaxed)
    }

    pub fn is_complete(&self) -> bool {
        matches!(*self.load_status.lock().unwrap(), Some(LoadStatus::Complete))
    }

    pub fn url(&self) -> Option<String> {
        self.state.lock().unwrap().url.clone()
    }

    pub fn title(&self) -> Option<String> {
        self.state.lock().unwrap().title.clone()
    }

    pub fn console(&self) -> Vec<String> {
        self.state.lock().unwrap().console.clone()
    }

    pub fn crash(&self) -> Option<String> {
        self.state.lock().unwrap().crash.clone()
    }

    /// Phase 3.9: page weight proxy — resource requests this document made.
    pub fn page_requests(&self) -> u64 {
        self.page_requests.load(Ordering::Relaxed)
    }

    /// Phase 4.3.4: record observable page activity (now).
    pub fn touch(&self) {
        let now = unix_now_ms();
        // v2.1 Phase 1.2: anchor the first activity of this document.
        let _ =
            self.first_activity_ms.compare_exchange(0, now, Ordering::Relaxed, Ordering::Relaxed);
        self.last_activity_ms.store(now, Ordering::Relaxed);
    }

    /// v2.1 Phase 1.2: reset the activity anchors for a new document
    /// (called from the navigation request path).
    pub fn begin_navigation(&self) {
        self.first_activity_ms.store(0, Ordering::Relaxed);
        self.last_activity_ms.store(0, Ordering::Relaxed);
        *self.embedder_complete_at.lock().unwrap() = None;
        *self.complete_at.lock().unwrap() = None;
    }

    /// v2.1 Phase 1.2: runner-loop gate — true once the page is
    /// embedder-complete (see `completion`) AND has stayed settled for
    /// `settle_ms` after the criterion first fired. Latches the anchor
    /// on first observation so the settle window is deterministic.
    pub fn settled_after_completion(
        &self,
        quiet_ms: u64,
        max_wait_ms: u64,
        settle_ms: u64,
    ) -> bool {
        let (embedder_done, _, _) = self.completion(quiet_ms, max_wait_ms);
        if !embedder_done {
            return false;
        }
        let mut slot = self.embedder_complete_at.lock().unwrap();
        let anchored = slot.unwrap_or_else(|| {
            let now = Instant::now();
            *slot = Some(now);
            now
        });
        anchored.elapsed() >= Duration::from_millis(settle_ms)
    }

    /// v2.1 Phase 1.2 — the embedder completion criterion.
    ///
    /// `LoadStatus::Complete` fires on the document `load` event, which a
    /// hung third-party ad iframe blocks indefinitely (measured: MDN
    /// never completed in a 45 s window while rendering fine). Chrome
    /// distinguishes `load` from `networkidle`; so do we, plus a hard
    /// cap so forever-polling pages cannot stall a harness:
    ///
    /// - `engine-load-event`: the engine fired `LoadStatus::Complete`
    ///   (also reported raw as `all_resources_complete`);
    /// - `quiet-period`: the document has been active (painted or made
    ///   requests) and nothing observable happened for `quiet_ms`;
    /// - `first-activity-cap`: `max_wait_ms` passed since the first
    ///   observable activity (painted pages only — a page that never
    ///   rendered still waits for the engine/timeout).
    ///
    /// Returns `(embedder_complete, raw_engine_complete, criterion)`.
    pub fn completion(&self, quiet_ms: u64, max_wait_ms: u64) -> (bool, bool, &'static str) {
        let raw = self.is_complete();
        if raw {
            return (true, true, "engine-load-event");
        }
        let first = self.first_activity_ms.load(Ordering::Relaxed);
        if first == 0 {
            return (false, false, "no-activity-yet");
        }
        let now = unix_now_ms();
        let last = self.last_activity_ms.load(Ordering::Relaxed);
        if now.saturating_sub(last) >= quiet_ms {
            return (true, false, "quiet-period");
        }
        if now.saturating_sub(first) >= max_wait_ms {
            return (true, false, "first-activity-cap");
        }
        (false, false, "pending")
    }

    /// Phase 4.3.4: ms since the last observable page activity. Returns
    /// None if the page has never been active (e.g. never painted).
    pub fn ms_since_activity(&self) -> Option<u64> {
        let last = self.last_activity_ms.load(Ordering::Relaxed);
        if last == 0 {
            return None;
        }
        Some(unix_now_ms().saturating_sub(last))
    }
}

/// The brows12 delegate.
pub struct HostDelegate {
    pub state: Arc<HostState>,
    pub privacy: Arc<PrivacyHost>,
    /// Set when a crash or fatal embedder error must stop the run.
    pub fatal: Arc<AtomicBool>,
}

impl HostDelegate {
    pub fn new(state: Arc<HostState>, privacy: Arc<PrivacyHost>) -> Rc<Self> {
        Rc::new(Self { state, privacy, fatal: Arc::new(AtomicBool::new(false)) })
    }

    /// uBO-style phase-2 cosmetic filtering: collect the document's class
    /// and id attributes, match them against EasyList's generic rules in
    /// the filter engine, and queue the hide-stylesheet JS (executed by the
    /// host loop — evaluation callbacks cannot evaluate re-entrantly).
    fn apply_class_id_cosmetics(&self, webview: &WebView) {
        let privacy = self.privacy.clone();
        let state = self.state.clone();
        webview.evaluate_javascript(crate::privacy::DOM_INFO_JS, move |result| {
            let Ok(servo::JSValue::String(info)) = result else {
                return;
            };
            let selectors = privacy.hidden_selectors_for_page(&info);
            if selectors.is_empty() {
                return;
            }
            privacy.cosmetic_pages_filtered.fetch_add(1, Ordering::Relaxed);
            let css = selectors.join(",\n");
            let js = crate::privacy::hide_stylesheet_js(&css);
            state.pending_js.lock().unwrap().push(js);
        });
    }
}

impl WebViewDelegate for HostDelegate {
    fn notify_url_changed(&self, _webview: WebView, url: Url) {
        self.state.state.lock().unwrap().url = Some(url.to_string());
    }

    fn notify_page_title_changed(&self, _webview: WebView, title: Option<String>) {
        self.state.state.lock().unwrap().title = title;
    }

    fn notify_load_status_changed(&self, webview: WebView, status: LoadStatus) {
        if matches!(status, LoadStatus::Complete) {
            *self.state.complete_at.lock().unwrap() = Some(Instant::now());
            // Phase 4 Area 2.1: generic class/id cosmetic rules need the
            // page's DOM attributes (uBO two-phase protocol). Round-trip
            // through evaluate_javascript, match in Rust, hide via JS.
            self.apply_class_id_cosmetics(&webview);
            // Phase 4.4.5: capture the page snippet for tab search.
            let state = self.state.clone();
            webview.evaluate_javascript(SNIPPET_JS, move |result| {
                if let Ok(servo::JSValue::String(s)) = result {
                    *state.page_snippet.lock().unwrap() = if s.is_empty() { None } else { Some(s) };
                }
            });
        }
        *self.state.load_status.lock().unwrap() = Some(status);
    }

    fn notify_new_frame_ready(&self, _webview: WebView) {
        self.state.frames.fetch_add(1, Ordering::Relaxed);
        // Phase 4.3.4: a new frame is observable activity — rAF loops,
        // CSS animations and timer-driven repaints all land here. The
        // budget governor uses it to spot idle-timer tabs in the
        // background.
        self.state.touch();
    }

    fn notify_animating_changed(&self, _webview: WebView, animating: bool) {
        self.state.animating.store(animating, Ordering::Relaxed);
    }

    fn notify_crashed(&self, _webview: WebView, reason: String, _backtrace: Option<String>) {
        self.state.state.lock().unwrap().crash = Some(reason);
        self.fatal.store(true, Ordering::Relaxed);
    }

    fn show_console_message(&self, _webview: WebView, _level: ConsoleLogLevel, message: String) {
        let mut st = self.state.state.lock().unwrap();
        if st.console.len() < 256 {
            st.console.push(message);
        }
    }

    fn request_navigation(&self, _webview: WebView, navigation: NavigationRequest) {
        // Phase 4 Area 2.4: HTTPS-Only is enforced in `load_web_resource`:
        // plain-HTTP documents are answered with a meta-refresh upgrade
        // page (a real subsequent navigation renders reliably, unlike
        // deny+reload or intercepted document redirects) and plain-HTTP
        // subresources with a 301 to their https:// URL.

        // Phase 3.9: a new document starts from a fresh page-weight count.
        self.state.page_requests.store(0, Ordering::Relaxed);
        // v2.1 Phase 1.2: fresh activity anchors for the completion
        // criterion (first-activity cap + quiet period).
        self.state.begin_navigation();
        // Phase 4.4.7: a new document starts with unknown media state.
        self.state.media_playing.store(false, Ordering::Relaxed);
        // Install cosmetic filters + scriptlets (+$csp meta) for the
        // destination document before it loads (UCM changes apply to the
        // next document).
        self.privacy.set_page_filtering(navigation.url.as_str());

        // Phase 4 Area 2.3: interstitial redirect-chain guard. Track
        // cross-domain navigation hops in an 8 s window; the 5th distinct
        // registrable domain in that window is an ad-interstitial funnel,
        // not human browsing (the initial load is not a hop). Hops are compared against the LAST RECORDED hop (not
        // the delegate's current-URL state, which may already reflect the
        // in-flight navigation by callback time).
        let target_domain =
            brows12_storage::registrable_domain(navigation.url.host_str().unwrap_or_default());
        let mut deny = false;
        let mut prev_domain = String::new();
        if !target_domain.is_empty() {
            let mut hops = self.state.nav_hops.lock().unwrap();
            let now = Instant::now();
            hops.retain(|(t, _)| now.duration_since(*t) < std::time::Duration::from_secs(8));
            prev_domain = hops.last().map(|(_, d)| d.clone()).unwrap_or_default();
            if !prev_domain.is_empty() && target_domain != prev_domain {
                if hops.len() >= 3 {
                    deny = true;
                } else {
                    hops.push((now, target_domain.clone()));
                }
            } else if prev_domain.is_empty() {
                hops.push((now, target_domain.clone()));
            }
        }

        if deny {
            self.privacy.record_redirect_chain_blocked(&prev_domain, &target_domain);
            navigation.deny();
            return;
        }

        // Default policy: allow. Popup policy hooks in `request_create_new`.
        navigation.allow();
    }

    /// Phase 4 Area 2.3: pop-up blocker. `window.open()` (and pop-unders,
    /// which are the same API opened behind the current window) creates an
    /// auxiliary webview through this hook; ignoring the request means no
    /// webview is created. brows12 blocks all auxiliary webviews by
    /// default — stricter than Chrome's gesture heuristic — because the
    /// embedder cannot observe user activation. Future UI: per-site
    /// exceptions.
    fn request_create_new(&self, parent_webview: WebView, _request: CreateNewWebViewRequest) {
        let source = parent_webview.url().map(|u| u.to_string()).unwrap_or_default();
        self.privacy.record_popup_blocked(&source);
    }

    /// Phase 4.4.7: media-session playback state — the audio/video
    /// "is playing" signal that exempts a tab from idle suspension.
    fn notify_media_session_event(&self, _webview: WebView, event: MediaSessionEvent) {
        if let MediaSessionEvent::PlaybackStateChange(state) = event {
            self.state
                .media_playing
                .store(matches!(state, MediaSessionPlaybackState::Playing), Ordering::Relaxed);
        }
    }

    fn load_web_resource(&self, _webview: WebView, load: WebResourceLoad) {
        let request = &load.request;
        let url_str = request.url.as_str().to_string();
        let source = request
            .referrer_url
            .as_ref()
            .map(|u| u.host_str().unwrap_or_default().to_string())
            .unwrap_or_default();
        let kind = destination_to_kind(&request.destination);

        // Phase 3.9: every resource the page asks for counts toward its
        // weight, blocked or not (blocked ones still cost a cache slot).
        self.state.page_requests.fetch_add(1, Ordering::Relaxed);
        // Phase 4.3.4: a resource request is observable activity too
        // (fetch/XHR-driven idle timers poll this way).
        self.state.touch();

        // v2.1 Phase 1.1: staged-load diagnostic deny (measurement only).
        // Precedes the privacy verdict so privacy counters stay clean;
        // answers with an empty 200 exactly like the block path so the
        // page sees a settled resource and the load can still complete.
        if crate::diag::denied(kind) {
            let response = WebResourceResponse::new(request.url.clone());
            load.intercept(response).finish();
            return;
        }

        // Phase 4 Area 2.5: CNAME-cloaking classification of document
        // hosts over DoH (cached 5 min; fails open). Runs here because the
        // initial navigation does not pass through request_navigation.
        if matches!(kind, RequestKind::Document) {
            self.privacy.check_cname_cloaking(&url_str);
        }

        let verdict = self.privacy.verdict(&url_str, &source, kind);

        if let Some(reason) = verdict.block {
            self.privacy.record_block(reason, &url_str);
            if let Some(res) = verdict.redirect {
                // $redirect: serve the real replacement resource body so
                // the page sees a plausible asset (keeps layout/JS happy).
                self.privacy.redirects_served.fetch_add(1, Ordering::Relaxed);
                let response = WebResourceResponse::new(request.url.clone())
                    .status_code(StatusCode::OK)
                    .status_message(b"OK".to_vec());
                let mut intercepted = load.intercept(response);
                intercepted.send_body_data(res.body);
                intercepted.finish();
            } else {
                // Answer with an empty 200 so the page sees a settled
                // resource instead of a network error.
                let response = WebResourceResponse::new(request.url.clone());
                load.intercept(response).finish();
            }
            return;
        }

        if let Some(rewritten) = verdict.rewritten_url {
            // $removeparam: strip tracking params via a permanent redirect.
            if let Ok(target) = url::Url::parse(&rewritten) {
                self.privacy.params_stripped.fetch_add(1, Ordering::Relaxed);
                let mut headers = HeaderMap::new();
                headers.insert(header::LOCATION, rewritten.parse().unwrap());
                let response = WebResourceResponse::new(target)
                    .status_code(StatusCode::MOVED_PERMANENTLY)
                    .status_message(b"Moved Permanently".to_vec())
                    .headers(headers);
                load.intercept(response).finish();
                return;
            }
        }

        if let Some(directives) = verdict.csp_directives {
            // $csp on an iframe response: we cannot add response headers on
            // the pass-through path, so record for the report. The
            // document-level meta injection happens in `set_page_filtering`.
            self.privacy.record_csp(&url_str, &directives);
        }

        // Phase 4 Area 2.4: HTTPS-Only. Exempt hosts never reach here
        // (upgrade() returns the URL unchanged).
        if request.url.scheme() == "http" {
            if let Some(upgraded) = self.privacy.upgrader.upgrade(&url_str) {
                if upgraded != url_str {
                    if matches!(kind, RequestKind::Document | RequestKind::Subdocument) {
                        // Documents: serve a meta-refresh upgrade page.
                        // Servo does not reliably render after deny+reload
                        // or an intercepted document redirect, but a
                        // meta-refresh issues a REAL navigation that
                        // renders normally.
                        let body = upgrade_page_html(&upgraded);
                        let mut headers = HeaderMap::new();
                        headers.insert(
                            header::CONTENT_TYPE,
                            "text/html; charset=utf-8".parse().unwrap(),
                        );
                        let response = WebResourceResponse::new(request.url.clone())
                            .status_code(StatusCode::OK)
                            .status_message(b"OK".to_vec())
                            .headers(headers);
                        let mut intercepted = load.intercept(response);
                        intercepted.send_body_data(body.into_bytes());
                        intercepted.finish();
                    } else {
                        // Subresources: 301 to the https:// version (the
                        // re-request re-enters this hook with https).
                        if let Ok(target) = url::Url::parse(&upgraded) {
                            let mut headers = HeaderMap::new();
                            headers.insert(header::LOCATION, upgraded.parse().unwrap());
                            let response = WebResourceResponse::new(target)
                                .status_code(StatusCode::MOVED_PERMANENTLY)
                                .status_message(b"Moved Permanently".to_vec())
                                .headers(headers);
                            load.intercept(response).finish();
                            return;
                        }
                    }
                    return;
                }
            }
        }

        // Phase 4 Area 2.7: response-header security guard.
        //
        // (a) Clickjacking defense: cross-origin frames are embedded only
        // if the frame target's own headers allow it (X-Frame-Options /
        // CSP frame-ancestors, probed + cached). Same-origin frames pass.
        if matches!(kind, RequestKind::Subdocument) {
            if let Some(parent) = request.referrer_url.as_ref() {
                if let Some(why) = self.privacy.security.check_frame(parent.as_str(), &url_str) {
                    self.privacy.security.record_frame_blocked(&url_str, &why);
                    let response = WebResourceResponse::new(request.url.clone());
                    load.intercept(response).finish();
                    return;
                }
            }
        }

        // (b) CSP enforcement on subresources: the document site's probed
        // policy (script-src / style-src / img-src / connect-src / …) is
        // evaluated with the Servo-team `content_security_policy` crate.
        // The main-frame document itself has no parent CSP to consult.
        if !matches!(kind, RequestKind::Document) {
            if let Some(parent) = request.referrer_url.as_ref() {
                if let Some(why) = self.privacy.security.check_subresource(
                    parent.as_str(),
                    &url_str,
                    &request.destination,
                ) {
                    self.privacy.security.record_csp_blocked(&url_str, &why);
                    let response = WebResourceResponse::new(request.url.clone());
                    load.intercept(response).finish();
                    return;
                }
            }
        }

        // (c) Mixed content: an https document asking for a plain-http
        // subresource that the upgrader did NOT rewrite (exempt host /
        // HTTPS-Only off) is blocked outright — matches Chrome's
        // post-M79 all-mixed-content blocking. Reaches here only when the
        // upgrade branch above did not early-return.
        if request.url.scheme() == "http" {
            if let Some(parent) = request.referrer_url.as_ref() {
                if parent.scheme() == "https"
                    && self.privacy.security.check_mixed_content(parent.as_str(), &url_str)
                {
                    self.privacy.security.record_mixed_content_blocked(&url_str);
                    let response = WebResourceResponse::new(request.url.clone());
                    load.intercept(response).finish();
                    return;
                }
            }
        }

        // (d) Header probe for the main-frame document (cached 5 min):
        // parses CSP / XFO / frame-ancestors / HSTS / COOP-COEP-CORP and
        // feeds runtime HSTS into the upgrader. Runs synchronously in the
        // document's own load hook, so every later subresource sees the
        // policy already cached.
        if matches!(kind, RequestKind::Document) && request.is_for_main_frame {
            self.privacy.security.probe_document(&url_str);
        }

        // Not blocked: let Servo load normally.
        drop(load);
    }
}

/// The upgrade page served for plain-HTTP document navigations.
fn upgrade_page_html(target: &str) -> String {
    let escaped = target.replace('&', "&amp;").replace('"', "&quot;");
    format!(
        "<!DOCTYPE html><html><head><meta charset=\"utf-8\">\
<meta http-equiv=\"refresh\" content=\"0;url={escaped}\">\
<title>Upgrading connection</title></head>\
<body style=\"font-family:sans-serif;padding:2em;color:#444\">\
Upgrading to a secure connection…</body></html>"
    )
}
