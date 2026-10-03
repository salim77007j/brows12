//! The brows12 WebView delegate: observes load progress, frames, console
//! output and (from Phase 1.5) intercepts web resources for the privacy
//! layer.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Instant;

use servo::{ConsoleLogLevel, LoadStatus, WebView, WebViewDelegate, WebResourceLoad};
use url::Url;

#[derive(Default)]
pub struct DelegateState {
    url: Option<String>,
    title: Option<String>,
    console: Vec<String>,
    blocked: Vec<String>,
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
    pub started_at: Mutex<Option<Instant>>,
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
        matches!(
            *self.load_status.lock().unwrap(),
            Some(LoadStatus::Complete)
        )
    }

    pub fn url(&self) -> Option<String> {
        self.state.lock().unwrap().url.clone()
    }

    pub fn title(&self) -> Option<String> {
        self.state.lock().unwrap().title.clone()
    }

    pub fn blocked(&self) -> Vec<String> {
        self.state.lock().unwrap().blocked.clone()
    }

    pub fn console(&self) -> Vec<String> {
        self.state.lock().unwrap().console.clone()
    }

    pub fn crash(&self) -> Option<String> {
        self.state.lock().unwrap().crash.clone()
    }
}

/// The brows12 delegate. All hooks are observational in Phase 1;
/// resource interception for the privacy layer lands in 1.5.
pub struct HostDelegate {
    pub state: std::sync::Arc<HostState>,
}

impl HostDelegate {
    pub fn new(state: std::sync::Arc<HostState>) -> Rc<Self> {
        Rc::new(Self { state })
    }
}

use std::rc::Rc;

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
    }

    fn show_console_message(&self, _webview: WebView, _level: ConsoleLogLevel, message: String) {
        let mut st = self.state.state.lock().unwrap();
        if st.console.len() < 256 {
            st.console.push(message);
        }
    }

    fn load_web_resource(&self, _webview: WebView, load: WebResourceLoad) {
        // Phase 1.5 will consult the privacy matcher here. For now the
        // load is dropped un-intercepted, which lets Servo fetch it.
        drop(load);
    }
}
