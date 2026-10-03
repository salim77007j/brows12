//! Phase 2 performance harness: headless multi-tab runner that measures
//! exactly what the mission gates demand (docs/SERVO_INTEGRATION_PLAN.md
//! §5), with `/proc` ground truth and Servo's own memory reporter.
//!
//! Scenarios (combinable in one run, driven by `PerfConfig`):
//! - `startup`  — ServoBuilder::build() wall time + time to first frame.
//! - per-tab    — RSS after each page settles (marginal cost per tab) and
//!   load-complete wall time.
//! - `suspend`  — hibernate background tabs (throttle + hide + drop the
//!   WebView → CloseWebView frees the pipeline) and report the RSS delta.
//! - `idle`     — CPU% over a window where the host loop only wakes on
//!   engine events (1 s safety poll), the embedder-idle analog of the
//!   shell's event-driven loop.
//! - `scroll`   — wheel events at 60 Hz into the last tab, frames counted
//!   via notify_new_frame_ready, rendered every tick.
//!
//! Run through `brows-perf` (bin) which prints JSON to stdout.

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::mpsc::channel;
use std::time::{Duration, Instant};

use serde::Serialize;
use servo::profile_traits::mem::{MemoryReport, MemoryReportResult, ReportKind};
use servo::{
    RenderingContext, Servo, ServoBuilder, SoftwareRenderingContext, WebView, WebViewBuilder,
    WheelDelta, WheelEvent, WheelMode,
};
use servo_base::generic_channel::GenericCallback;

use crate::delegate::{HostDelegate, HostState};
use crate::privacy::PrivacyHost;
use crate::waker::CondvarWaker;

#[derive(Clone, Debug)]
pub struct PerfConfig {
    pub urls: Vec<url::Url>,
    pub width: u32,
    pub height: u32,
    pub settle_ms: u64,
    pub timeout_ms: u64,
    /// Hibernate background tabs after all loads settle.
    pub suspend: bool,
    /// Idle-CPU measurement window (0 = skip).
    pub idle_secs: u64,
    /// Scroll benchmark duration on the last tab (0 = skip).
    pub scroll_secs: u64,
    /// Ask the engine for its malloc-size-of report. OFF by default:
    /// in servo 0.6.0 the SystemFontService panics (usize add overflow in
    /// servo-malloc-size-of) while collecting its report — an upstream
    /// bug discovered during Phase 2; patch + PR planned (Phase 4.5).
    /// RSS (/proc) remains the primary metric for every gate.
    pub engine_report: bool,
}

impl Default for PerfConfig {
    fn default() -> Self {
        PerfConfig {
            urls: vec![url::Url::parse("https://example.com/").unwrap()],
            width: 1280,
            height: 800,
            settle_ms: 1_200,
            timeout_ms: 60_000,
            suspend: false,
            idle_secs: 0,
            scroll_secs: 0,
            engine_report: false,
        }
    }
}

#[derive(Serialize, Debug, Clone)]
pub struct StartupReport {
    /// Wall time for ServoBuilder::build() (engine init: WebRender,
    /// constellation, font cache, SpiderMonkey runtime).
    pub servo_build_ms: u128,
    /// Wall time from run start to the first engine frame of tab 1.
    pub first_frame_ms: u128,
    /// Wall time until tab 1 reached LoadStatus::Complete.
    pub first_load_complete_ms: Option<u128>,
}

#[derive(Serialize, Debug, Clone)]
pub struct TabMemoryReport {
    pub url: String,
    pub complete: bool,
    pub load_complete_ms: Option<u128>,
    /// Process RSS (KiB) right after this tab settled.
    pub rss_after_kb: u64,
}

#[derive(Serialize, Debug, Clone)]
pub struct EngineMemoryReport {
    /// Sum of every Explicit* report, bytes, per process.
    pub explicit_total_bytes: u64,
    /// PID → explicit bytes.
    pub by_pid: Vec<(u32, u64)>,
    /// Top 12 explicit report paths by size (aggregated), bytes.
    pub top_paths: Vec<(String, u64)>,
}

#[derive(Serialize, Debug, Clone)]
pub struct SuspensionReport {
    /// Background tabs hibernated (WebView dropped).
    pub hibernated: usize,
    pub rss_before_kb: u64,
    pub rss_after_kb: u64,
    pub rss_freed_kb: u64,
}

#[derive(Serialize, Debug, Clone)]
pub struct IdleReport {
    pub window_secs: f64,
    /// Average CPU utilization over the window (100% = one core).
    pub cpu_percent: f64,
    pub rss_kb: u64,
}

#[derive(Serialize, Debug, Clone)]
pub struct ScrollReport {
    pub url: String,
    pub window_secs: f64,
    pub frames: u64,
    /// Frames per second delivered by the engine under the host loop.
    pub fps: f64,
    /// Software-GL caveat marker (Xvfb/swrast in CI; real GPU on desktop).
    pub software_gl: bool,
}

#[derive(Serialize, Debug, Clone)]
pub struct PerfReport {
    pub engine: String,
    pub config: PerfConfigSer,
    pub startup: Option<StartupReport>,
    pub tabs: Vec<TabMemoryReport>,
    pub memory_report: Option<EngineMemoryReport>,
    pub suspension: Option<SuspensionReport>,
    pub idle: Option<IdleReport>,
    pub scroll: Option<ScrollReport>,
    pub rss_start_kb: u64,
    pub rss_final_kb: Option<u64>,
    pub peak_rss_kb: Option<u64>,
}

/// PerfConfig minus the urls (they are echoed per-tab) — serializable.
#[derive(Serialize, Debug, Clone)]
pub struct PerfConfigSer {
    pub tabs: usize,
    pub width: u32,
    pub height: u32,
    pub suspend: bool,
    pub idle_secs: u64,
    pub scroll_secs: u64,
    pub engine_report: bool,
}

/// Entry point for `brows-perf`.
pub fn run_perf(config: PerfConfig) -> PerfReport {
    let started = Instant::now();
    let rss_start_kb = crate::metrics::rss_kb().unwrap_or(0);

    // ---- Engine boot (measured) ----------------------------------------
    let build_started = Instant::now();
    let context: Rc<dyn RenderingContext> = Rc::new(
        SoftwareRenderingContext::new(winit::dpi::PhysicalSize::new(config.width, config.height))
            .expect("SoftwareRenderingContext (headless perf path)"),
    );
    let waker = CondvarWaker::default();
    let servo: Servo = ServoBuilder::default()
        .event_loop_waker(Box::new(waker.clone()))
        .preferences(crate::prefs::brows12_preferences())
        .build();
    let servo_build_ms = build_started.elapsed().as_millis();

    let privacy = PrivacyHost::new();
    let ucm = Rc::new(servo::UserContentManager::new(&servo));
    privacy.set_user_content_manager(ucm.clone());

    // ---- Tabs ------------------------------------------------------------
    struct Slot {
        webview: Option<WebView>,
        state: std::sync::Arc<HostState>,
        // Kept alive next to the webview; dropped with it on hibernation.
        _ctx: Rc<dyn RenderingContext>,
    }
    let mut slots: Vec<Slot> = Vec::new();
    let mut tabs: Vec<TabMemoryReport> = Vec::new();
    let mut first_frame_ms = 0u128;
    let mut first_load_complete_ms = None;

    for url in config.urls.clone() {
        let state = std::sync::Arc::new(HostState::new());
        let delegate = HostDelegate::new(state.clone(), privacy.clone());
        let webview = WebViewBuilder::new(&servo, context.clone())
            .url(url.clone())
            .delegate(delegate)
            .user_content_manager(ucm.clone())
            .build();
        webview.focus();
        webview.load(url.clone());
        slots.push(Slot { webview: Some(webview), state: state.clone(), _ctx: context.clone() });

        // Load + settle this tab before opening the next (isolates the
        // per-tab RSS marginal).
        let tab_deadline = Instant::now() + Duration::from_millis(config.timeout_ms);
        loop {
            servo.spin_event_loop();
            if first_frame_ms == 0 && state.frame_count() > 0 {
                first_frame_ms = started.elapsed().as_millis();
            }
            if state.is_complete() && settled(&state, config.settle_ms) {
                break;
            }
            if state.crash().is_some() || Instant::now() > tab_deadline {
                break;
            }
            // While loading we poll at 125 Hz like the Phase 1 runner.
            waker.wait_timeout(8);
        }
        if first_load_complete_ms.is_none() {
            first_load_complete_ms = state
                .complete_at
                .lock()
                .unwrap()
                .map(|t| t.duration_since(started).as_millis());
        }
        // Let the engine drain before sampling RSS.
        for _ in 0..10 {
            servo.spin_event_loop();
            waker.wait_timeout(8);
        }
        tabs.push(TabMemoryReport {
            url: url.to_string(),
            complete: state.is_complete(),
            load_complete_ms: state
                .complete_at
                .lock()
                .unwrap()
                .map(|t| t.duration_since(started).as_millis()),
            rss_after_kb: crate::metrics::rss_kb().unwrap_or(0),
        });
    }

    // ---- Engine memory report (create_memory_report hook) ----------------
    let memory_report = if config.engine_report {
        request_engine_memory_report(&servo, &waker)
    } else {
        None
    };

    // ---- Suspension (hibernate background tabs) --------------------------
    let suspension = if config.suspend && slots.len() > 1 {
        let rss_before = crate::metrics::rss_kb().unwrap_or(0);
        let mut hibernated = 0usize;
        let bg_count = slots.len().saturating_sub(1);
        for slot in slots.iter_mut().take(bg_count) {
            if let Some(webview) = slot.webview.take() {
                // Hibernation protocol: stop timers/animations, remove from
                // the compositor scene, then drop the handle — Drop sends
                // CloseWebView and the constellation tears the pipeline down.
                webview.set_throttled(true);
                webview.hide();
                drop(webview);
                hibernated += 1;
            }
        }
        // Give the constellation time to process the closes and free.
        let drain_until = Instant::now() + Duration::from_millis(2_500);
        while Instant::now() < drain_until {
            servo.spin_event_loop();
            waker.wait_timeout(16);
        }
        let rss_after = crate::metrics::rss_kb().unwrap_or(0);
        Some(SuspensionReport {
            hibernated,
            rss_before_kb: rss_before,
            rss_after_kb: rss_after,
            rss_freed_kb: rss_before.saturating_sub(rss_after),
        })
    } else {
        None
    };

    // ---- Idle CPU window --------------------------------------------------
    let idle = if config.idle_secs > 0 {
        let meter = crate::metrics::CpuMeter::start();
        let until = Instant::now() + Duration::from_secs(config.idle_secs);
        while Instant::now() < until {
            servo.spin_event_loop();
            // 1 s safety poll: engine activity arrives via the waker; the
            // poll only bounds wake-up latency, it does not drive work.
            waker.wait_timeout(1_000);
        }
        Some(IdleReport {
            window_secs: meter.elapsed_secs(),
            cpu_percent: meter.percent().unwrap_or(0.0),
            rss_kb: crate::metrics::rss_kb().unwrap_or(0),
        })
    } else {
        None
    };

    // ---- Scroll benchmark (last tab, 60 Hz wheel input) -------------------
    let scroll = if config.scroll_secs > 0 {
        let url = config
            .urls
            .last()
            .cloned()
            .unwrap_or_else(|| url::Url::parse("about:blank").unwrap())
            .to_string();
        let frames0 = slots.last().map(|s| s.state.frame_count()).unwrap_or(0);
        let meter = crate::metrics::CpuMeter::start();
        let until = Instant::now() + Duration::from_secs(config.scroll_secs);
        while Instant::now() < until {
            servo.spin_event_loop();
            if let Some(slot) = slots.last() {
                if let Some(webview) = &slot.webview {
                    webview.notify_input_event(servo::input_events::InputEvent::Wheel(
                        WheelEvent::new(
                            WheelDelta { x: 0.0, y: -76.0, z: 0.0, mode: WheelMode::DeltaPixel },
                            servo::DevicePoint::new(
                                config.width as f32 / 2.0,
                                config.height as f32 / 2.0,
                            )
                            .into(),
                        ),
                    ));
                    webview.paint();
                }
            }
            waker.wait_timeout(16);
        }
        let frames1 = slots.last().map(|s| s.state.frame_count()).unwrap_or(0);
        let window = meter.elapsed_secs();
        let frames = frames1.saturating_sub(frames0);
        Some(ScrollReport {
            url,
            window_secs: window,
            frames,
            fps: if window > 0.0 { frames as f64 / window } else { 0.0 },
            software_gl: true,
        })
    } else {
        None
    };

    let rss_final_kb = crate::metrics::rss_kb();
    let peak_rss_kb = crate::metrics::peak_rss_kb();

    PerfReport {
        engine: crate::headless::engine_id(),
        config: PerfConfigSer {
            tabs: slots.len(),
            width: config.width,
            height: config.height,
            suspend: config.suspend,
            idle_secs: config.idle_secs,
            scroll_secs: config.scroll_secs,
            engine_report: config.engine_report,
        },
        startup: Some(StartupReport {
            servo_build_ms,
            first_frame_ms,
            first_load_complete_ms,
        }),
        tabs,
        memory_report,
        suspension,
        idle,
        scroll,
        rss_start_kb,
        rss_final_kb,
        peak_rss_kb,
    }
}

fn settled(state: &std::sync::Arc<HostState>, settle_ms: u64) -> bool {
    if let Some(t) = *state.complete_at.lock().unwrap() {
        return t.elapsed() >= Duration::from_millis(settle_ms);
    }
    false
}

/// Ask the constellation for its malloc-size-of report and wait (while
/// spinning the loop) for the callback to fire.
fn request_engine_memory_report(servo: &Servo, waker: &CondvarWaker) -> Option<EngineMemoryReport> {
    let (tx, rx) = channel::<Vec<MemoryReport>>();
    let cb = GenericCallback::<MemoryReportResult>::new(
        move |result: Result<MemoryReportResult, ipc_channel::IpcError>| {
            if let Ok(report) = result {
                let _ = tx.send(report.results);
            }
        },
    )
    .expect("memory report callback");
    servo.create_memory_report(cb);
    let mut reports: Option<Vec<MemoryReport>> = None;
    let deadline = Instant::now() + Duration::from_secs(10);
    while reports.is_none() && Instant::now() < deadline {
        servo.spin_event_loop();
        if let Ok(r) = rx.try_recv() {
            reports = Some(r);
            break;
        }
        waker.wait_timeout(8);
    }
    let all = reports?;
    let mut by_pid: HashMap<u32, u64> = HashMap::new();
    let mut top: HashMap<String, u64> = HashMap::new();
    let mut total = 0u64;
    for proc_report in all {
        let entry = by_pid.entry(proc_report.pid).or_insert(0);
        for r in proc_report.reports {
            if matches!(
                r.kind,
                ReportKind::ExplicitJemallocHeapSize
                    | ReportKind::ExplicitSystemHeapSize
                    | ReportKind::ExplicitNonHeapSize
                    | ReportKind::ExplicitUnknownLocationSize
            ) {
                let size = r.size as u64;
                *entry += size;
                total += size;
                let path = r.path.join("/");
                *top.entry(path).or_insert(0) += size;
            }
        }
    }
    let mut top_paths: Vec<(String, u64)> = top.into_iter().collect();
    top_paths.sort_by_key(|(_, v)| std::cmp::Reverse(*v));
    top_paths.truncate(12);
    let mut by_pid: Vec<(u32, u64)> = by_pid.into_iter().collect();
    by_pid.sort_by_key(|(_, v)| std::cmp::Reverse(*v));
    Some(EngineMemoryReport { explicit_total_bytes: total, by_pid, top_paths })
}
