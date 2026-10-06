//! Headless render mode: load a URL in a Servo WebView, wait for settle,
//! capture a PNG, and emit a JSON report. Runs under Xvfb (GLX) or, when
//! a window cannot be created, attempts a pure-software rendering context.

use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::Serialize;
use servo::{RenderingContext, Servo, ServoBuilder, WebView, WebViewBuilder};
use winit::application::ApplicationHandler;
use winit::dpi::PhysicalSize;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy};
use winit::raw_window_handle::{HasDisplayHandle, HasWindowHandle};

use crate::capture::capture_webview;
use crate::delegate::{env_complete_max_wait_ms, env_complete_quiet_ms, HostDelegate, HostState};
use crate::gfx::GfxPolicy;
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
    /// v2.1 Phase 4 GPU support: force the software (CPU) rendering lane
    /// (`--software` flag or the `gfx.software-rendering` pref).
    pub software: bool,
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
            software: false,
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
    /// v2.1 Phase 1.2: raw engine flag (`LoadStatus::Complete`).
    pub all_resources_complete: bool,
    /// v2.1 Phase 1.2: which completion criterion decided `complete`.
    pub complete_criterion: &'static str,
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
    /// CNAME-cloaked hosts detected via DoH.
    pub cname_cloaks: u64,
    /// DoH queries made by the browser itself.
    pub doh_queries: u64,
    /// Aggregated privacy dashboard (Area 2.8 data layer).
    pub dashboard: crate::dashboard::PrivacyDashboard,
    /// Area 2.7 security guard: header probes performed.
    pub security_probes: u64,
    /// Frames denied by XFO / frame-ancestors.
    pub frames_blocked: u64,
    /// Subresource requests blocked by CSP enforcement.
    pub csp_blocked: u64,
    /// Plain-http subresources blocked on https documents.
    pub mixed_content_blocked: u64,
    /// HSTS policies learned at runtime from response headers.
    pub hsts_learned: u64,
    pub coop_observed: u64,
    pub coep_observed: u64,
    pub corp_observed: u64,
    /// Frame-guard decision log (capped).
    pub frame_log: Vec<String>,
    /// CSP violation log (capped).
    pub csp_block_log: Vec<String>,
}

/// Entry point for `brows render --engine servo`.
pub fn run_headless(config: HeadlessConfig) -> HeadlessReport {
    crate::init_crypto_provider();
    let started = Instant::now();
    let policy = GfxPolicy::from_flags(config.software);

    // Software (no-X) lane first: works where surfman can create an
    // offscreen EGL context (user-space libEGL), zero window needed. The
    // attempt is panic-guarded by the gfx factory (docs/GPU_SUPPORT.md).
    // Under ForceSoftware this is the ONLY lane — if it fails we report
    // and return instead of trying the windowed path.
    {
        let size = PhysicalSize::new(config.width, config.height);
        match crate::gfx::create_software_context(size) {
            Ok(selected) => {
                eprintln!("brows12 gfx: headless on the software lane `{}`", selected.backend);
                let ctx: Rc<dyn RenderingContext> = selected.context;
                return run_without_window(config, ctx, started);
            }
            Err(attempts) => {
                // Distinguish "software lane failed" from "forced software
                // but lane failed": in the latter case there is nothing to
                // fall back to — report and stop.
                if policy == GfxPolicy::ForceSoftware {
                    eprint!(
                        "{}",
                        crate::gfx::fatal_report(
                            &attempts,
                            "Software rendering was forced (--software or \
                             gfx.software-rendering=true) but its lane failed.",
                        )
                    );
                    let mut report = HeadlessReport::error_stub(&config, started);
                    report.error = Some("no software rendering backend available".into());
                    return report;
                }
                eprintln!(
                    "brows12 gfx: software lane unavailable — falling through to the windowed path"
                );
            }
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
    app.result.unwrap_or_else(|| {
        HeadlessReport::zero_report(&config, started, "event loop ended without capture")
    })
}

pub fn engine_id() -> String {
    "servo 0.6.0 (stylo 0.21, webrender 0.70, spidermonkey 153, brows12 v2)".into()
}

impl HeadlessReport {
    /// All-zero report for early-abort paths: the event loop ended without
    /// capture, or no rendering backend was available at all.
    pub(crate) fn zero_report(
        config: &HeadlessConfig,
        started: Instant,
        error: &'static str,
    ) -> HeadlessReport {
        HeadlessReport {
            engine: engine_id(),
            url: config.url.to_string(),
            final_url: None,
            title: None,
            width: config.width,
            height: config.height,
            frames: 0,
            complete: false,
            all_resources_complete: false,
            complete_criterion: "no-activity-yet",
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
                cname_cloaks: 0,
                doh_queries: 0,
                dashboard: crate::dashboard::PrivacyDashboard {
                    ads_blocked: 0,
                    trackers_blocked: 0,
                    blocked_sample: vec![],
                    rules_loaded: 0,
                    redirects_served: 0,
                    params_stripped: 0,
                    csp_injections: 0,
                    scriptlets_injected: 0,
                    cosmetic_pages_filtered: 0,
                    fingerprint_pages_protected: 0,
                    popups_blocked: 0,
                    redirect_chains_blocked: 0,
                    https_upgrades: 0,
                    hsts_hits: 0,
                    hsts_learned: 0,
                    doh_queries: 0,
                    cname_cloaks: 0,
                    cookies: Default::default(),
                    security_probes: 0,
                    frames_blocked: 0,
                    csp_blocked: 0,
                    mixed_content_blocked: 0,
                    coop_observed: 0,
                    coep_observed: 0,
                    corp_observed: 0,
                    frame_log: vec![],
                    csp_block_log: vec![],
                    popup_log: vec![],
                    cloak_log: vec![],
                },
                security_probes: 0,
                frames_blocked: 0,
                csp_blocked: 0,
                mixed_content_blocked: 0,
                hsts_learned: 0,
                coop_observed: 0,
                coep_observed: 0,
                corp_observed: 0,
                frame_log: vec![],
                csp_block_log: vec![],
            },
            load_complete_ms: None,
            total_ms: started.elapsed().as_millis(),
            png: None,
            error: Some(error.into()),
        }
    }

    /// Minimal all-zero report for early-abort paths (e.g. forced-software
    /// mode with no software backend available).
    pub(crate) fn error_stub(config: &HeadlessConfig, started: Instant) -> HeadlessReport {
        HeadlessReport::zero_report(config, started, "rendering backend unavailable")
    }
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
        if state.settled_after_completion(
            env_complete_quiet_ms(),
            env_complete_max_wait_ms(),
            config.settle_ms,
        ) {
            break;
        }
        if started.elapsed().as_millis() as u64 > config.timeout_ms || state.crash().is_some() {
            break;
        }
        waker.wait_timeout(8);
    }

    finish(&config, &servo, &webview, &context, &state, &privacy, started)
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
    let mut report = HeadlessReport {
        engine: engine_id(),
        url: config.url.to_string(),
        final_url: state.url(),
        title: state.title(),
        width: config.width,
        height: config.height,
        frames: state.frame_count(),
        complete: state.completion(env_complete_quiet_ms(), env_complete_max_wait_ms()).0,
        all_resources_complete: state.is_complete(),
        complete_criterion: state.completion(env_complete_quiet_ms(), env_complete_max_wait_ms()).2,
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
            cname_cloaks: privacy.cname_cloaks.load(std::sync::atomic::Ordering::Relaxed),
            doh_queries: privacy.doh.query_count(),
            dashboard: crate::dashboard::PrivacyDashboard {
                ads_blocked: 0,
                trackers_blocked: 0,
                blocked_sample: vec![],
                rules_loaded: 0,
                redirects_served: 0,
                params_stripped: 0,
                csp_injections: 0,
                scriptlets_injected: 0,
                cosmetic_pages_filtered: 0,
                fingerprint_pages_protected: 0,
                popups_blocked: 0,
                redirect_chains_blocked: 0,
                https_upgrades: 0,
                hsts_hits: 0,
                hsts_learned: 0,
                doh_queries: 0,
                cname_cloaks: 0,
                cookies: Default::default(),
                security_probes: 0,
                frames_blocked: 0,
                csp_blocked: 0,
                mixed_content_blocked: 0,
                coop_observed: 0,
                coep_observed: 0,
                corp_observed: 0,
                frame_log: vec![],
                csp_block_log: vec![],
                popup_log: vec![],
                cloak_log: vec![],
            },
            security_probes: 0,
            frames_blocked: 0,
            csp_blocked: 0,
            mixed_content_blocked: 0,
            hsts_learned: 0,
            coop_observed: 0,
            coep_observed: 0,
            corp_observed: 0,
            frame_log: vec![],
            csp_block_log: vec![],
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
    // Phase 4 Area 2.7: merge the security guard's counters + logs.
    let security = privacy.security.summary();
    report.privacy.security_probes = security.probes;
    report.privacy.frames_blocked = security.frames_blocked;
    report.privacy.csp_blocked = security.csp_blocked;
    report.privacy.mixed_content_blocked = security.mixed_content_blocked;
    report.privacy.hsts_learned = security.hsts_learned;
    report.privacy.coop_observed = security.coop_observed;
    report.privacy.coep_observed = security.coep_observed;
    report.privacy.corp_observed = security.corp_observed;
    report.privacy.frame_log = security.frame_log;
    report.privacy.csp_block_log = security.csp_block_log;
    // Phase 4 Area 2.8: aggregated privacy dashboard.
    report.privacy.dashboard = crate::dashboard::snapshot(privacy);
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
        let window = Arc::new(
            event_loop
                .create_window(winit::window::Window::default_attributes())
                .expect("failed to create X window"),
        );
        let size = PhysicalSize::new(self.config.width, self.config.height);
        let _ = window.request_inner_size(size);
        // v2.1 Phase 4 GPU support: the windowed lane goes through the gfx
        // factory (panic-guarded, logged). Reaching this point means the
        // software lane already failed in run_headless (a successful
        // software context returns before the event loop is built), so the
        // hardware lane is the only remaining option. If it also fails,
        // report cleanly and stop instead of panicking.
        let ctx: Rc<dyn RenderingContext> =
            match crate::gfx::create_window_parent_context(window.clone(), size) {
                Ok(selected) => selected.context,
                Err(attempts) => {
                    eprint!(
                        "{}",
                        crate::gfx::fatal_report(
                            &attempts,
                            "All rendering lanes failed for this headless run.",
                        )
                    );
                    self.result = Some(HeadlessReport::zero_report(
                        &self.config,
                        self.started,
                        "no rendering backend available",
                    ));
                    event_loop.exit();
                    return;
                }
            };

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
                || state.settled_after_completion(
                    env_complete_quiet_ms(),
                    env_complete_max_wait_ms(),
                    self.config.settle_ms,
                )
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
