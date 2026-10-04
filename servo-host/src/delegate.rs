//! The brows12 WebView delegate: observes load progress, frames, console
//! output, intercepts web resources for the privacy layer (Phase 1.5),
//! and prepares cosmetic filters at navigation time.

use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use http::header;
use http::{HeaderMap, StatusCode};
use servo::{
    ConsoleLogLevel, CreateNewWebViewRequest, LoadStatus, NavigationRequest, WebResourceLoad,
    WebResourceResponse, WebView, WebViewDelegate,
};
use url::Url;

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
        }
        *self.state.load_status.lock().unwrap() = Some(status);
    }

    fn notify_new_frame_ready(&self, _webview: WebView) {
        self.state.frames.fetch_add(1, Ordering::Relaxed);
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
        // Phase 3.9: a new document starts from a fresh page-weight count.
        self.state.page_requests.store(0, Ordering::Relaxed);
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

        // Not blocked: let Servo load normally.
        drop(load);
    }
}
