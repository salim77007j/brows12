//! The brows12 WebView delegate: observes load progress, frames, console
//! output, intercepts web resources for the privacy layer (Phase 1.5),
//! and prepares cosmetic filters at navigation time.

use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use servo::{
    ConsoleLogLevel, LoadStatus, NavigationRequest, WebView, WebViewDelegate, WebResourceLoad,
    WebResourceResponse,
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
}

impl HostState {
    pub fn new() -> Self {
        Self::default()
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
        Rc::new(Self {
            state,
            privacy,
            fatal: Arc::new(AtomicBool::new(false)),
        })
    }
}

impl WebViewDelegate for HostDelegate {
    fn notify_url_changed(&self, _webview: WebView, url: Url) {
        self.state.state.lock().unwrap().url = Some(url.to_string());
    }

    fn notify_page_title_changed(&self, _webview: WebView, title: Option<String>) {
        self.state.state.lock().unwrap().title = title;
    }

    fn notify_load_status_changed(&self, _webview: WebView, status: LoadStatus) {
        if matches!(status, LoadStatus::Complete) {
            *self.state.complete_at.lock().unwrap() = Some(Instant::now());
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
        // Install cosmetic filters for the destination document before it
        // loads (UCM stylesheet changes apply to the next document).
        self.privacy.set_cosmetic_filters(navigation.url.as_str());
        // Default policy: allow. Popup/ads policy decisions can hook here.
        navigation.allow();
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

        if let Some(reason) = self.privacy.should_block(&url_str, &source, kind) {
            self.privacy.record_block(reason, &url_str);
            // Answer with an empty 200 so the page sees a settled
            // resource instead of a network error.
            let response = WebResourceResponse::new(request.url.clone());
            load.intercept(response).finish();
            return;
        }
        // Not blocked: let Servo load normally.
        drop(load);
    }
}
