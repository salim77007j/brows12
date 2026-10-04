//! Headless render mode: load a URL in a Servo WebView, wait for settle,
//! capture a PNG, and emit a JSON report. Runs under Xvfb (GLX) or, when
//! a window cannot be created, attempts a pure-software rendering context.

use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::Serialize;
use servo::{
    RenderingContext, Servo, ServoBuilder, SoftwareRenderingContext, WebView, WebViewBuilder,
    WindowRenderingContext,
};
use winit::application::ApplicationHandler;
use winit::dpi::PhysicalSize;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy};
use winit::raw_window_handle::{HasDisplayHandle, HasWindowHandle};

use crate::capture::capture_webview;
use crate::delegate::{HostDelegate, HostState};
use crate::waker::{HostWakerEvent, ProxyWaker};

#[derive(Clone, Debug)]
pub struct HeadlessConfig {
    pub url: url::Url,
    pub width: u32,
    pub height: u32,
    pub png: Option<PathBuf>,
    pub json: Option<PathBuf>,
    /// Hard wall-clock budget for the whole load+settle.
    pub timeout_ms: u64,
    /// How long frames must stay quiet after LoadStatus::Complete
    /// before we consider the page settled.
    pub settle_ms: u64,
}

impl Default for HeadlessConfig {
    fn default() -> Self {
        Self {
            url: url::Url::parse("about:blank").unwrap(),
            width: 1280,
            height: 800,
            png: None,
            json: None,
            timeout_ms: 60_000,
            settle_ms: 1_200,
        }
    }
}

#[derive(Serialize, Debug)]
pub struct HeadlessReport {
    pub engine: String,
    pub url: String,
    pub final_url: Option<String>,
    pub title: Option<String>,
    pub width: u32,
    pub height: u32,
    pub frames: u64,
    pub complete: bool,
    pub crashed: bool,
    pub crash_reason: Option<String>,
    pub console_messages: Vec<String>,
    pub privacy: PrivacySummary,
    pub load_complete_ms: Option<u128>,
    pub total_ms: u128,
    pub png: Option<String>,
    pub error: Option<String>,
}

/// Privacy shield results for this run (Phase 1.5 / Phase 4 Area 2).
#[derive(Serialize, Debug, Clone)]
pub struct PrivacySummary {
    pub enabled: bool,
    pub ads_blocked: u64,
    pub trackers_blocked: u64,
    pub blocked_requests: Vec<String>,
    /// Requests answered with a `$redirect` replacement resource.
    pub redirects_served: u64,
    /// Requests rewritten via `$removeparam`.
    pub params_stripped: u64,
    /// `$csp` directive sets surfaced (documents + iframes).
    pub csp_injections: u64,
    /// Pages that received uBO scriptlet code this session.
    pub scriptlets_injected: u64,
    /// Rules loaded into the filter engine.
    pub rules_loaded: usize,
    /// `$csp` recording log (capped).
    pub csp_log: Vec<String>,
    /// http:// navigations + subresources upgraded to https://.
    pub https_upgrades: u64,
    /// HSTS (preload + cache) enforcement hits.
    pub hsts_hits: u64,
    /// Pop-ups blocked (window.open / pop-unders).
    pub popups_blocked: u64,
    /// Interstitial redirect chains cut off.
    pub redirect_chains_blocked: u64,
}

/// Entry point for `brows render --engine servo`.
pub fn run_headless(config: HeadlessConfig) -> HeadlessReport {
    let started = Instant::now();

    // Software (no-X) path first: works where surfman can create an
    // offscreen EGL context (user-space libEGL), zero window needed.
    match SoftwareRenderingContext::new(PhysicalSize::new(config.width, config.height)) {
        Ok(soft) => {
            let ctx: Rc<dyn RenderingContext> = Rc::new(soft);
            return run_without_window(config, ctx, started);
        }
        Err(_) => {
            // Fall through to the winit/Xvfb (GLX) path below.
        }
    }

    let event_loop = EventLoop::with_user_event()
        .build()
        .expect("Failed to create winit event loop (is Xvfb running?)");
    let proxy = event_loop.create_proxy();
    let mut app = HeadlessApp {
        config: config.clone(),
        proxy,
        started,
        state: None,
        privacy: None,
        servo: None,
        webview: None,
        context: None,
        capture_done: false,
        result: None,
    };
    event_loop.run_app(&mut app).expect("event loop failed");
    app.result.unwrap_or_else(|| HeadlessReport {
        engine: engine_id(),
        url: config.url.to_string(),
        final_url: None,
        title: None,
        width: config.width,
        height: config.height,
        frames: 0,
        complete: false,
        crashed: false,
        crash_reason: None,
        console_messages: vec![],
        privacy: PrivacySummary {
            enabled: false,
            ads_blocked: 0,
            trackers_blocked: 0,
            blocked_requests: vec![],
            redirects_served: 0,
            params_stripped: 0,
            csp_injections: 0,
            scriptlets_injected: 0,
            rules_loaded: 0,
            csp_log: vec![],
            https_upgrades: 0,
            hsts_hits: 0,
            popups_blocked: 0,
            redirect_chains_blocked: 0,
        },
        load_complete_ms: None,
        total_ms: started.elapsed().as_millis(),
        png: None,
        error: Some("event loop ended without capture".into()),
    })
}

pub fn engine_id() -> String {
    "servo 0.6.0 (stylo 0.21, webrender 0.70, spidermonkey 153, brows12 v2)".into()
}

fn run_without_window(
    config: HeadlessConfig,
    context: Rc<dyn RenderingContext>,
    started: Instant,
) -> HeadlessReport {
    let state = Arc::new(HostState::new());
    let privacy = crate::privacy::PrivacyHost::new();
    let waker = crate::waker::CondvarWaker::default();
    let servo: Servo = ServoBuilder::default()
        .event_loop_waker(Box::new(waker.clone()))
        .preferences(crate::prefs::compat_preferences())
        .build();
    let ucm = Rc::new(servo::UserContentManager::new(&servo));
    privacy.set_user_content_manager(ucm.clone());
    let delegate = HostDelegate::new(state.clone(), privacy.clone());
    let webview = WebViewBuilder::new(&servo, context.clone())
        .url(config.url.clone())
        .delegate(delegate)
        .user_content_manager(ucm)
        .build();
    webview.focus();
    webview.load(config.url.clone());

    loop {
        servo.spin_event_loop();
        // Run follow-up JS queued from evaluation callbacks (e.g. the
        // phase-2 cosmetic hide stylesheet).
        for js in state.drain_pending_js() {
            webview.evaluate_javascript(js, |_| {});
        }
        if state.is_complete() && settled(&state, config.settle_ms) {
            break;
        }
        if started.elapsed().as_millis() as u64 > config.timeout_ms || state.crash().is_some() {
            break;
        }
        waker.wait_timeout(8);
    }

    finish(&config, &servo, &webview, &context, &state, &privacy, started)
}

fn settled(state: &Arc<HostState>, settle_ms: u64) -> bool {
    if let Some(t) = *state.complete_at.lock().unwrap() {
        return t.elapsed() >= Duration::from_millis(settle_ms);
    }
    false
}

#[allow(clippy::too_many_arguments)]
fn finish(
    config: &HeadlessConfig,
    servo: &Servo,
    webview: &WebView,
    context: &Rc<dyn RenderingContext>,
    state: &Arc<HostState>,
    privacy: &Arc<crate::privacy::PrivacyHost>,
    started: Instant,
) -> HeadlessReport {
    let img = capture_webview(webview, context);
    let png_path = match (&config.png, &img) {
        (Some(path), Some(img)) => {
            crate::capture::save_png(img, path).ok().map(|_| path.display().to_string())
        }
        _ => None,
    };
    let (ads_blocked, trackers_blocked, blocked_requests) = privacy.blocked_summary();
    let csp_log = privacy.csp_log.lock().unwrap().clone();
    let report = HeadlessReport {
        engine: engine_id(),
        url: config.url.to_string(),
        final_url: state.url(),
        title: state.title(),
        width: config.width,
        height: config.height,
        frames: state.frame_count(),
        complete: state.is_complete(),
        crashed: state.crash().is_some(),
        crash_reason: state.crash(),
        console_messages: state.console(),
        privacy: PrivacySummary {
            enabled: privacy.enabled.load(std::sync::atomic::Ordering::Relaxed),
            ads_blocked,
            trackers_blocked,
            blocked_requests,
            redirects_served: privacy.redirects_served.load(std::sync::atomic::Ordering::Relaxed),
            params_stripped: privacy.params_stripped.load(std::sync::atomic::Ordering::Relaxed),
            csp_injections: privacy.csp_injections.load(std::sync::atomic::Ordering::Relaxed),
            scriptlets_injected: privacy
                .scriptlets_injected
                .load(std::sync::atomic::Ordering::Relaxed),
            rules_loaded: privacy.blocker.rules_loaded,
            csp_log,
            https_upgrades: privacy.upgrader.upgrade_count(),
            hsts_hits: privacy.upgrader.hsts_hit_count(),
            popups_blocked: privacy.popups_blocked.load(std::sync::atomic::Ordering::Relaxed),
            redirect_chains_blocked: privacy
                .redirect_chains_blocked
                .load(std::sync::atomic::Ordering::Relaxed),
        },
        load_complete_ms: state
            .complete_at
            .lock()
            .unwrap()
            .map(|t| t.duration_since(started).as_millis()),
        total_ms: started.elapsed().as_millis(),
        png: png_path,
        error: img.is_none().then(|| "capture produced no image".to_string()),
    };
    if let Some(json_path) = &config.json {
        if let Ok(text) = serde_json::to_string_pretty(&report) {
            let _ =
                std::fs::create_dir_all(json_path.parent().unwrap_or(std::path::Path::new(".")));
            let _ = std::fs::write(json_path, text);
        }
    }
    // Give Servo a clean shutdown.
    let _ = servo;
    report
}

struct HeadlessApp {
    config: HeadlessConfig,
    proxy: EventLoopProxy<HostWakerEvent>,
    started: Instant,
    state: Option<Arc<HostState>>,
    privacy: Option<Arc<crate::privacy::PrivacyHost>>,
    servo: Option<Servo>,
    webview: Option<WebView>,
    context: Option<Rc<dyn RenderingContext>>,
    capture_done: bool,
    result: Option<HeadlessReport>,
}

impl ApplicationHandler<HostWakerEvent> for HeadlessApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.servo.is_some() {
            return;
        }
        let display_handle =
            event_loop.display_handle().expect("no display handle (run under xvfb-run)");
        let window = event_loop
            .create_window(winit::window::Window::default_attributes())
            .expect("failed to create X window");
        let window_handle = window.window_handle().expect("no window handle");
        let size = PhysicalSize::new(self.config.width, self.config.height);
        let _ = window.request_inner_size(size);
        let wctx = WindowRenderingContext::new(display_handle, window_handle, size)
            .expect("WindowRenderingContext (GLX) failed");
        let ctx: Rc<dyn RenderingContext> = Rc::new(wctx);

        let state = Arc::new(HostState::new());
        let privacy = crate::privacy::PrivacyHost::new();
        let waker = ProxyWaker::new(self.proxy.clone());
        let servo: Servo = ServoBuilder::default()
            .event_loop_waker(Box::new(waker))
            .preferences(crate::prefs::compat_preferences())
            .build();
        let ucm = Rc::new(servo::UserContentManager::new(&servo));
        privacy.set_user_content_manager(ucm.clone());
        let delegate = HostDelegate::new(state.clone(), privacy.clone());
        let webview = WebViewBuilder::new(&servo, ctx.clone())
            .url(self.config.url.clone())
            .delegate(delegate)
            .user_content_manager(ucm)
            .build();
        webview.focus();
        webview.load(self.config.url.clone());

        // Periodic tick so settle checks run even without external events.
        let tick_proxy = self.proxy.clone();
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_millis(16));
            if tick_proxy.send_event(HostWakerEvent).is_err() {
                return;
            }
        });

        self.state = Some(state);
        self.privacy = Some(privacy);
        self.servo = Some(servo);
        self.webview = Some(webview);
        self.context = Some(ctx);
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, _event: HostWakerEvent) {
        if let (Some(servo), Some(state), Some(webview)) =
            (&self.servo, &self.state.as_ref(), &self.webview)
        {
            servo.spin_event_loop();
            for js in state.drain_pending_js() {
                webview.evaluate_javascript(js, |_| {});
            }
            let done = self.capture_done
                || (state.is_complete() && settled(state, self.config.settle_ms))
                || self.started.elapsed().as_millis() as u64 > self.config.timeout_ms
                || state.crash().is_some();
            if done && !self.capture_done {
                self.capture_done = true;
                let ctx = self.context.as_ref().unwrap();
                let privacy = self.privacy.as_ref().unwrap();
                let report =
                    finish(&self.config, servo, webview, ctx, state, privacy, self.started);
                self.result = Some(report);
                _event_loop.exit();
            }
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: winit::window::WindowId,
        event: WindowEvent,
    ) {
        if matches!(event, WindowEvent::CloseRequested) {
            event_loop.exit();
            return;
        }
        // Treat any window traffic as loop traffic.
        self.user_event(event_loop, HostWakerEvent);
    }
}
