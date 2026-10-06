//! brows12-ui (v2) — the real browser shell on top of the Servo engine.
//!
//! Architecture: Servo runs on the UI thread (its WebViews are
//! main-thread-bound, like winit). Each tab owns a `WebView` bound to an
//! offscreen rendering context; on every redraw we spin Servo's event
//! loop, paint the active webview, read the framebuffer back, and
//! composite the tiny-skia chrome (tab strip + toolbar + omnibox) over it
//! with softbuffer presentation. Mouse/keyboard input below the chrome is
//! forwarded to the active webview in viewport coordinates.

mod chrome;
mod model;
mod text;

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use keyboard_types::{Code, Key as KKey, KeyState, KeyboardEvent, Location, Modifiers};
use servo::input_events::{InputEvent, MouseButtonAction, MouseButtonEvent, MouseMoveEvent};
use servo::{
    DeviceIntRect, DeviceIntSize, DevicePoint, KeyboardEvent as ServoKeyboardEvent,
    RenderingContext, Servo, ServoBuilder, SoftwareRenderingContext, UserContentManager, WebView,
    WebViewBuilder, WheelDelta, WheelEvent, WheelMode, WindowRenderingContext,
};
use servo_host::budget::{self, BudgetConfig, Degradation, JsHeapTier};
use servo_host::delegate::{HostDelegate, HostState};
use servo_host::memory::{self, GovernorConfig, Pressure};
use servo_host::privacy::PrivacyHost;
use servo_host::psi::{self, PsiConfig};
use servo_host::waker::HostWakerEvent;
use tiny_skia::{Pixmap, PremultipliedColorU8};
use url::Url;
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::dpi::PhysicalSize;
use winit::event::{
    ElementState, KeyEvent, MouseButton as WinitButton, MouseScrollDelta, WindowEvent,
};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowId};

use chrome::Hit;
use model::{InjectCmd, Status, UiTab, START_HTML, VIEWPORT_H, VIEWPORT_W};
use text::UiText;

fn main() {
    servo_host::init_crypto_provider();
    let software_flag = parse_cli_flags();
    let started = Instant::now();
    let event_loop = EventLoop::with_user_event().build().expect("event loop");
    event_loop.set_control_flow(ControlFlow::Wait);
    let mut app =
        App { proxy: Some(event_loop.create_proxy()), inner: None, started, software_flag };
    init_pending_channel();
    event_loop.run_app(&mut app).expect("run loop");
}

/// CLI parsing for the shell. `--software` forces the CPU rendering lane
/// (see docs/GPU_SUPPORT.md); unrecognized arguments are warned about and
/// ignored so the window still opens.
fn parse_cli_flags() -> bool {
    let mut software = false;
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "--software" => software = true,
            "--help" | "-h" => {
                println!(
                    "brows12-ui — the brows12 browser shell\n\
                     Usage: brows12-ui [flags]\n\
                     \n\
                     Flags:\n\
                     --software   Force software (CPU) rendering; skip every GPU lane.\n\
                     --help       Show this help.\n\
                     \n\
                     Preferences (env):\n\
                     BROWS12_SET_PREF=\"gfx.software-rendering=true\"  same as --software"
                );
                std::process::exit(0);
            }
            other => {
                eprintln!("brows12-ui: ignoring unrecognized argument `{other}`");
            }
        }
    }
    software
}

/// The rendering parent context, per resolved backend lane.
///
/// `Window` is the hardware lane (ANGLE/D3D11 on Windows, EGL/GLX on
/// Linux); `Software` is the CPU lane (D3D11 WARP on Windows, Mesa
/// llvmpipe on Linux). Webviews bind to contexts derived from either.
enum ParentCtx {
    Window(Rc<WindowRenderingContext>),
    Software(Rc<SoftwareRenderingContext>),
}

struct App {
    inner: Option<Gui>,
    proxy: Option<EventLoopProxy<HostWakerEvent>>,
    /// Process start, for the startup metrics report.
    started: Instant,
    /// `--software` CLI flag (OR-ed with the gfx.software-rendering pref).
    software_flag: bool,
}

/// Return free allocator pages to the OS (glibc `malloc_trim`).
/// No-op on non-glibc platforms (Windows, macOS) — v2.1 Phase 4: the
/// Windows/macOS CI builds compile the full UI shell, and these libc
/// symbols only exist on glibc.
#[cfg(target_os = "linux")]
fn malloc_trim_os() {
    unsafe {
        libc::malloc_trim(0);
    }
}
#[cfg(not(target_os = "linux"))]
fn malloc_trim_os() {}

/// glibc `malloc_stats` to stderr (BROWS12_MALLOC_STATS attribution).
/// No-op on non-glibc platforms.
#[cfg(target_os = "linux")]
fn malloc_stats_os() {
    unsafe {
        libc::malloc_stats();
    }
}
#[cfg(not(target_os = "linux"))]
fn malloc_stats_os() {}

/// Per-tab Servo runtime: the webview plus its rendering context and state.
/// `ctx` is the lane the webview was built on (an offscreen context derived
/// from the hardware parent, or the shared software context on the CPU lane).
struct TabRuntime {
    webview: WebView,
    state: Arc<HostState>,
    #[allow(dead_code)]
    ctx: Rc<dyn RenderingContext>,
    last_painted_frame: u64,
    /// Phase 4.3.1: ladder state — the tab was trimmed (hide + throttle
    /// + malloc_trim) at this instant, and escalation to hibernate only
    /// happens after the trim grace. Reset on restore (new runtime).
    trimmed_at: Option<Instant>,
}

#[allow(dead_code)]
struct Gui {
    window: Arc<Window>,
    context: softbuffer::Context<Arc<Window>>,
    surface: softbuffer::Surface<Arc<Window>, Arc<Window>>,
    proxy: EventLoopProxy<HostWakerEvent>,
    servo: Option<Servo>,
    parent_ctx: Option<ParentCtx>,
    ucm: Rc<UserContentManager>,
    privacy: Arc<PrivacyHost>,
    tabs: Vec<UiTab>,
    runtimes: HashMap<u64, TabRuntime>,
    next_tab_id: u64,
    /// Phase 4.4.3: tab groups data layer (name/color/collapsed +
    /// membership); exposed via FIFO commands and the strip color bar.
    groups: servo_host::tabgroups::TabGroupStore,
    /// Phase 4.4.4: where the "last session" file lives (None = no
    /// persistence). Restores happen at startup; saves on quit + every
    /// `BROWS12_SESSION_SAVE_SECS` (default 60).
    session_path: Option<std::path::PathBuf>,
    next_session_save_at: Option<Instant>,
    /// Phase 4.4.5: tab-search index (title/url/snippet per tab).
    search_index: servo_host::tabsearch::TabSearchIndex,
    /// Phase 4.4.7: the idle-suspend policy (never / N seconds / 60s).
    suspend_policy: servo_host::suspend::SuspendPolicy,
    /// Phase 4.4.7: user-level URL exemptions (substrings).
    suspend_exempt_urls: Vec<String>,
    active: usize,
    text: UiText,
    cursor: (f32, f32),
    hover: Hit,
    caret_on: bool,
    dirty: bool,
    quit: bool,
    blink_phase: u64,
    /// Bumped on every Loading→Loaded transition of the active tab; the
    /// validation snapshot hook saves one composited frame per generation.
    snapshot_gen: u64,
    // ---- Phase 2: memory governor + startup instrumentation --------------
    governor: GovernorConfig,
    next_governor_at: Instant,
    /// Total tabs hibernated by the governor this session.
    governor_hibernated: u64,
    // ---- Phase 4.3.1/3.4/3.6: per-tab budgets, JS-heap tiers, PSI --------
    budget: BudgetConfig,
    psi: PsiConfig,
    /// PSI-triggered reclaims respect this cooldown so a sustained spike
    /// reclaims at a humane pace instead of hibernating the whole strip.
    next_psi_reclaim_at: Instant,
    /// JS-heap tier currently applied to the engine (to avoid re-sending
    /// set_preference every tick).
    js_tier: JsHeapTier,
    /// v2.1 Phase 2: the one shared offscreen rendering context (see
    /// `offscreen_ctx`) — None until the first tab is created. On the
    /// software lane this is the shared software context itself.
    shared_ctx: Option<Rc<dyn RenderingContext>>,
    /// Wall time of ServoBuilder::build(), for BROWS12_UI_START_METRICS.
    servo_build_ms: u128,
    /// Wall time of the first presented frame (set once).
    first_present_ms: Option<u128>,
    started: Instant,
}

/// Marker prefix so the start page shows as `brows12://start` in the omnibox.
const START_MARKER: &str = "data:text/html;base64,";

fn start_page_url() -> Url {
    let b64 = base64_encode(START_HTML.as_bytes());
    Url::parse(&format!("{START_MARKER}{b64}")).expect("start url")
}

fn display_url(u: &str) -> String {
    if u.starts_with(START_MARKER) {
        "brows12://start".to_string()
    } else {
        u.to_string()
    }
}

impl ApplicationHandler<HostWakerEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.inner.is_some() {
            return;
        }
        let window = Arc::new(
            event_loop
                .create_window(
                    Window::default_attributes()
                        .with_title("brows12")
                        .with_inner_size(LogicalSize::new(chrome::WIN_W, chrome::WIN_H))
                        .with_resizable(false),
                )
                .expect("create window"),
        );

        let context = softbuffer::Context::new(window.clone()).expect("softbuffer context");
        let surface =
            softbuffer::Surface::new(&context, window.clone()).expect("softbuffer surface");

        let size = PhysicalSize::new(chrome::WIN_W, chrome::WIN_H);
        // v2.1 Phase 4 GPU support: resolve the rendering backend through
        // the resilient chain (docs/GPU_SUPPORT.md). Hardware lane first
        // unless --software / gfx.software-rendering forces the CPU lane;
        // every attempt is panic-guarded and logged; total failure exits
        // cleanly with an actionable report instead of panicking.
        let policy = servo_host::gfx::GfxPolicy::from_flags(self.software_flag);
        let parent_ctx = match policy {
            servo_host::gfx::GfxPolicy::Auto => {
                match servo_host::gfx::create_window_parent_context(window.clone(), size) {
                    Ok(selected) => ParentCtx::Window(selected.context),
                    Err(window_attempts) => match servo_host::gfx::create_software_context(size) {
                        Ok(selected) => {
                            eprintln!(
                                "brows12 gfx: hardware lane failed ({} attempt(s)) — \
                                     continuing on the software (CPU) lane `{}`",
                                window_attempts.len(),
                                selected.backend
                            );
                            ParentCtx::Software(selected.context)
                        }
                        Err(software_attempts) => {
                            let mut all = window_attempts;
                            all.extend(software_attempts);
                            eprint!(
                                "{}",
                                servo_host::gfx::fatal_report(
                                    &all,
                                    "Neither a GPU lane nor the CPU lane could initialize.",
                                )
                            );
                            std::process::exit(1);
                        }
                    },
                }
            }
            servo_host::gfx::GfxPolicy::ForceSoftware => {
                match servo_host::gfx::create_software_context(size) {
                    Ok(selected) => ParentCtx::Software(selected.context),
                    Err(attempts) => {
                        eprint!(
                            "{}",
                            servo_host::gfx::fatal_report(
                                &attempts,
                                "Software rendering was forced (--software or \
                             gfx.software-rendering=true) but its lane failed.",
                            )
                        );
                        std::process::exit(1);
                    }
                }
            }
        };

        let proxy = self.proxy.as_ref().expect("event loop proxy").clone();
        let waker = servo_host::waker::ProxyWaker::new(proxy.clone());
        let build_started = Instant::now();
        let servo: Servo = ServoBuilder::default()
            .event_loop_waker(Box::new(waker))
            .preferences({
                let mut prefs = servo_host::brows12_preferences();
                servo_host::prefs::apply_env_overrides(&mut prefs);
                prefs
            })
            .build();
        let servo_build_ms = build_started.elapsed().as_millis();

        let privacy = PrivacyHost::new();
        let ucm = Rc::new(UserContentManager::new(&servo));
        privacy.set_user_content_manager(ucm.clone());

        // Phase 4.3: one env parse, shared between the RSS governor and
        // the per-tab budget (BROWS12_MEM_BUDGET_MB is meaningful to both).
        let governor_cfg = GovernorConfig::from_env();
        let mut gui = Gui {
            window,
            context,
            surface,
            proxy,
            servo: Some(servo),
            parent_ctx: Some(parent_ctx),
            ucm,
            privacy,
            tabs: Vec::new(),
            runtimes: HashMap::new(),
            next_tab_id: 1,
            groups: servo_host::tabgroups::TabGroupStore::new(),
            session_path: session_path_from_env(),
            next_session_save_at: None,
            search_index: servo_host::tabsearch::TabSearchIndex::new(),
            suspend_policy: servo_host::suspend::SuspendPolicy::from_env(),
            suspend_exempt_urls: servo_host::suspend::exempt_urls_from_env(),
            active: 0,
            text: UiText::new(),
            cursor: (0.0, 0.0),
            hover: Hit::None,
            caret_on: true,
            dirty: true,
            quit: false,
            blink_phase: 0,
            snapshot_gen: 0,
            governor: governor_cfg.clone(),
            // Phase 4.4.1: the first tick waits out the warmup so session
            // setup (tab creation, activation history) completes before
            // the predictive reclaim starts ranking.
            next_governor_at: Instant::now() + governor_cfg.interval + governor_cfg.warmup,
            governor_hibernated: 0,
            budget: BudgetConfig {
                total_budget_kb: governor_cfg.budget_kb,
                ..BudgetConfig::default()
            },
            psi: PsiConfig::from_env(),
            next_psi_reclaim_at: Instant::now(),
            js_tier: JsHeapTier::Normal,
            shared_ctx: None,
            servo_build_ms,
            first_present_ms: None,
            started: self.started,
        };
        // Phase 4.4.4: initialize the automation event fifo BEFORE the
        // session restore, so `session_restored` / `session_live_tab`
        // events reach the harness.
        if let (Ok(_fifo), Ok(events)) =
            (std::env::var("BROWS12_UI_CMD_FIFO"), std::env::var("BROWS12_UI_EVENT_FIFO"))
        {
            model::init_event_fifo(&events);
        }
        gui.restore_session_or_new();
        let start_proxy = gui.proxy.clone();
        let start_wake = gui.window.clone();
        self.inner = Some(gui);
        // Phase 2.2: present the chrome immediately — do not wait for the
        // first engine frame to drive the first present.
        start_wake.request_redraw();

        // Automation channels (validation under Xvfb) — same protocol as v1.
        // The event fifo was initialized above (before session restore).
        if let Ok(fifo) = std::env::var("BROWS12_UI_CMD_FIFO") {
            model::emit(format!(
                "start pid={} viewport={}x{}",
                std::process::id(),
                VIEWPORT_W,
                VIEWPORT_H
            ));
            std::thread::spawn(move || {
                use std::io::BufRead;
                loop {
                    let Ok(file) = std::fs::File::open(&fifo) else { return };
                    for line in std::io::BufReader::new(file).lines().map_while(Result::ok) {
                        let cmd = if let Some(rest) = line.strip_prefix("<OMNI> ") {
                            Some(InjectCmd::Omni(rest.to_string()))
                        } else if let Some(rest) = line.strip_prefix("<TEXT> ") {
                            Some(InjectCmd::Text(rest.to_string()))
                        } else if let Some(rest) = line.strip_prefix("<SWITCH> ") {
                            rest.parse::<usize>().ok().map(InjectCmd::Switch)
                        } else if let Some(rest) = line.strip_prefix("<HIBERNATE> ") {
                            rest.parse::<usize>().ok().map(InjectCmd::Hibernate)
                        } else if let Some(rest) = line.strip_prefix("<RESTORE> ") {
                            rest.parse::<usize>().ok().map(InjectCmd::Restore)
                        } else if let Some(rest) = line.strip_prefix("<SCROLL> ") {
                            rest.parse::<f32>().ok().map(InjectCmd::Scroll)
                        } else if let Some(rest) = line.strip_prefix("<GROUP_NEW> ") {
                            let mut it = rest.splitn(2, '|');
                            match (it.next(), it.next()) {
                                (Some(n), Some(c)) => {
                                    Some(InjectCmd::GroupNew(n.to_string(), c.to_string()))
                                }
                                _ => None,
                            }
                        } else if let Some(rest) = line.strip_prefix("<GROUP_DEL> ") {
                            rest.parse::<u32>().ok().map(InjectCmd::GroupDel)
                        } else if let Some(rest) = line.strip_prefix("<GROUP_ADD> ") {
                            let mut it = rest.splitn(2, '|');
                            match (it.next(), it.next()) {
                                (Some(g), Some(t)) => {
                                    match (g.parse::<u32>(), t.parse::<usize>()) {
                                        (Ok(g), Ok(t)) => Some(InjectCmd::GroupAdd(g, t)),
                                        _ => None,
                                    }
                                }
                                _ => None,
                            }
                        } else if let Some(rest) = line.strip_prefix("<GROUP_REMOVE> ") {
                            rest.parse::<usize>().ok().map(InjectCmd::GroupRemove)
                        } else if let Some(rest) = line.strip_prefix("<GROUP_TOGGLE> ") {
                            rest.parse::<u32>().ok().map(InjectCmd::GroupToggle)
                        } else if line == "<GROUPS>" {
                            Some(InjectCmd::GroupsDump)
                        } else if let Some(rest) = line.strip_prefix("<TABSEARCH> ") {
                            Some(InjectCmd::TabSearch(rest.to_string()))
                        } else if let Some(rest) = line.strip_prefix("<PIN> ") {
                            rest.parse::<usize>().ok().map(InjectCmd::Pin)
                        } else if let Some(rest) = line.strip_prefix("<SLEEP> ") {
                            rest.parse::<u64>().ok().map(InjectCmd::Sleep)
                        } else {
                            match line.as_str() {
                                "<RETURN>" => Some(InjectCmd::Return),
                                "<MEMREPORT>" => Some(InjectCmd::MemReport),
                                "<BACK>" => Some(InjectCmd::Back),
                                "<FORWARD>" => Some(InjectCmd::Forward),
                                "<RELOAD>" => Some(InjectCmd::Reload),
                                "<NEWTAB>" => Some(InjectCmd::NewTab),
                                "<QUIT>" => Some(InjectCmd::Quit),
                                other => {
                                    if other.is_empty() {
                                        None
                                    } else {
                                        Some(InjectCmd::Text(other.to_string()))
                                    }
                                }
                            }
                        };
                        if let Some(InjectCmd::Sleep(ms)) = cmd {
                            std::thread::sleep(Duration::from_millis(ms));
                            continue;
                        }
                        if let Some(cmd) = cmd {
                            let sent =
                                PENDING_TX.get().map(|tx| tx.send(cmd).is_ok()).unwrap_or(false);
                            if sent && start_proxy.send_event(HostWakerEvent).is_ok() {
                                start_wake.request_redraw();
                            }
                        }
                    }
                }
            });
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, _event: HostWakerEvent) {
        if let Some(gui) = self.inner.as_mut() {
            if gui.quit {
                event_loop.exit();
                return;
            }
            gui.tick();
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let Some(gui) = self.inner.as_mut() else { return };
        match event {
            WindowEvent::CloseRequested => {
                // Phase 4.4.4: quit-time session save on window close.
                gui.save_session("close");
                event_loop.exit();
            }
            WindowEvent::RedrawRequested => {
                gui.tick();
                gui.draw();
            }
            WindowEvent::CursorMoved { position, .. } => {
                gui.cursor = (position.x as f32, position.y as f32);
                gui.hover = chrome::hit_test(gui.cursor.0, gui.cursor.1, gui.tabs.len());
                if gui.cursor.1 > chrome::CHROME_H {
                    gui.forward_mouse_move();
                }
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: WinitButton::Left,
                ..
            } => {
                if gui.cursor.1 > chrome::CHROME_H {
                    gui.forward_mouse_button(MouseButtonAction::Down);
                } else {
                    gui.click();
                }
            }
            WindowEvent::MouseInput {
                state: ElementState::Released,
                button: WinitButton::Left,
                ..
            } => {
                if gui.cursor.1 > chrome::CHROME_H {
                    gui.forward_mouse_button(MouseButtonAction::Up);
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let (dx, dy, mode) = match delta {
                    MouseScrollDelta::LineDelta(x, y) => {
                        ((x * 76.0) as f64, (y * 76.0) as f64, WheelMode::DeltaLine)
                    }
                    MouseScrollDelta::PixelDelta(p) => (p.x, p.y, WheelMode::DeltaPixel),
                };
                gui.forward_wheel(dx, dy, mode);
            }
            WindowEvent::KeyboardInput {
                event: KeyEvent { state: ElementState::Pressed, logical_key, text, .. },
                ..
            } => gui.key(logical_key, text.map(|s| s.to_string())),
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let Some(gui) = self.inner.as_mut() else { return };
        if gui.quit {
            event_loop.exit();
            return;
        }
        // Phase 2.3 — event-driven idle: pump the loop at 16 ms ONLY while
        // a tab is loading or the active page is animating (rAF,
        // transitions). Otherwise wait for engine events (ProxyWaker) or
        // the next governor tick. This is what makes idle CPU ~0.
        let busy = gui.any_busy();
        let mut next_wake =
            if busy { Some(Instant::now() + Duration::from_millis(16)) } else { None };
        // Phase 4.4.4: periodic session save (crash insurance between
        // quit-time saves).
        if let Some(at) = gui.next_session_save_at {
            if Instant::now() >= at {
                gui.save_session("periodic");
            }
            if let Some(next) = gui.next_session_save_at {
                next_wake = Some(match next_wake {
                    Some(t) => t.min(next),
                    None => next,
                });
            }
        }
        // Phase 2.1: the governor runs here — this is the ONLY callback
        // winit fires when a WaitUntil deadline expires on an idle system.
        if gui.governor.enabled && Instant::now() >= gui.next_governor_at {
            gui.governor_tick();
            gui.next_governor_at = Instant::now() + gui.governor.interval;
        }
        if gui.governor.enabled {
            next_wake = Some(match next_wake {
                Some(t) => t.min(gui.next_governor_at),
                None => gui.next_governor_at,
            });
        }
        event_loop.set_control_flow(match next_wake {
            Some(t) => ControlFlow::WaitUntil(t),
            None => ControlFlow::Wait,
        });
    }
}

static PENDING_TX: std::sync::OnceLock<std::sync::mpsc::Sender<InjectCmd>> =
    std::sync::OnceLock::new();
static PENDING_RX: std::sync::OnceLock<std::sync::Mutex<std::sync::mpsc::Receiver<InjectCmd>>> =
    std::sync::OnceLock::new();

fn init_pending_channel() {
    let (tx, rx) = std::sync::mpsc::channel::<InjectCmd>();
    let _ = PENDING_TX.set(tx);
    let _ = PENDING_RX.set(std::sync::Mutex::new(rx));
}

/// Phase 4.4.4: the "last session" file. Env `BROWS12_SESSION_FILE`;
/// unset means session persistence is off entirely.
fn session_path_from_env() -> Option<std::path::PathBuf> {
    std::env::var("BROWS12_SESSION_FILE")
        .ok()
        .filter(|s| !s.is_empty())
        .map(std::path::PathBuf::from)
}

/// Session save cadence. Env `BROWS12_SESSION_SAVE_SECS` (default 60;
/// 0 disables periodic saves — quit-time saves always happen).
fn session_save_interval() -> Duration {
    Duration::from_secs(
        std::env::var("BROWS12_SESSION_SAVE_SECS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(60),
    )
}

fn base64_encode(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [chunk[0], chunk.get(1).copied().unwrap_or(0), chunk.get(2).copied().unwrap_or(0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
}

impl Gui {
    // ---- Servo plumbing ---------------------------------------------------

    /// True while any tab is loading or the active page is animating —
    /// the condition for the 16 ms pump in `about_to_wait`.
    fn any_busy(&self) -> bool {
        let loading = self.tabs.iter().any(|t| t.status == Status::Loading);
        let animating = self
            .active_id()
            .and_then(|id| self.runtimes.get(&id))
            .map(|rt| rt.state.animating.load(std::sync::atomic::Ordering::Relaxed))
            .unwrap_or(false);
        loading || animating
    }

    fn tick(&mut self) {
        // Deliver queued automation commands through the normal handlers.
        if let Some(rx) = PENDING_RX.get() {
            if let Ok(rx) = rx.lock() {
                while let Ok(cmd) = rx.try_recv() {
                    self.inject(cmd);
                }
            }
        }
        if let Some(servo) = self.servo.as_ref() {
            servo.spin_event_loop();
            // Drain follow-up JS queued by evaluation callbacks (phase-2
            // cosmetic hide stylesheet) for the active tab.
            if let Some(id) = self.active_id() {
                if let Some(rt) = self.runtimes.get(&id) {
                    for js in rt.state.drain_pending_js() {
                        rt.webview.evaluate_javascript(js, |_| {});
                    }
                }
            }
        }
        self.sync_active_tab_state();
        // Repaint when the active page produced new frames.
        let new_frames = self
            .active_id()
            .and_then(|id| self.runtimes.get(&id))
            .map(|rt| rt.state.frame_count() != rt.last_painted_frame)
            .unwrap_or(false);
        if new_frames {
            self.dirty = true;
        }
        if self.dirty {
            self.window.request_redraw();
        }
    }

    /// v2.1 Phase 2 — `<MEMREPORT>` diagnostics: issue the engine memory
    /// report and emit the result as one `memreport` event. Runs
    /// synchronously with a bounded inline spin (harness-only command;
    /// blocking the UI loop for ≤5 s is acceptable and keeps the
    /// automation protocol simple — the governor tick is too slow to
    /// poll from).
    fn run_mem_report(&mut self) {
        let Some(servo) = self.servo.as_ref() else { return };
        let sink: std::sync::Arc<std::sync::Mutex<Option<servo_host::perf::EngineMemoryReport>>> =
            std::sync::Arc::new(std::sync::Mutex::new(None));
        servo_host::perf::request_engine_memory_report_async(servo, sink.clone());
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            servo.spin_event_loop();
            if sink.lock().map(|s| s.is_some()).unwrap_or(true) {
                break;
            }
            std::thread::sleep(Duration::from_millis(8));
        }
        if let Ok(mut slot) = sink.lock() {
            if let Some(report) = slot.take() {
                let rss_kb = servo_host::metrics::rss_kb().unwrap_or(0);
                let suspended = self.tabs.iter().filter(|t| t.suspended).count();
                let live = self.runtimes.len();
                model::emit(format!(
                    "memreport rss_kb={rss_kb} live_runtimes={live} suspended={suspended} \
                     engine_report={}",
                    serde_json::to_string(&report).unwrap_or_else(|_| "{}".into()),
                ));
            }
        };
    }

    fn active_id(&self) -> Option<u64> {
        self.tabs.get(self.active)?.id
    }

    fn sync_active_tab_state(&mut self) {
        let Some(id) = self.active_id() else { return };
        let Some(rt) = self.runtimes.get(&id) else { return };
        let complete = rt.state.is_complete();
        let url = rt.state.url().map(|u| display_url(&u));
        let title = rt.state.title();
        let t = &mut self.tabs[self.active];
        let mut changed = false;
        // The pre-navigation about:blank document also reports Complete;
        // never let it into session history (it would corrupt hibernation
        // restore URLs and back/forward).
        let recordable = url.as_deref().map(|u| u != "about:blank").unwrap_or(false);
        if complete && t.status == Status::Loading {
            t.status = Status::Loaded;
            if let Some(title) = title.clone() {
                t.title = title;
            }
            if recordable {
                let u = url.clone().unwrap();
                if t.history.last().map(String::as_str) != Some(u.as_str()) {
                    t.history.truncate(t.hindex + 1);
                    t.history.push(u.clone());
                    t.hindex = t.history.len() - 1;
                }
            }
            model::emit(format!(
                "loaded tab={} url={} title={}",
                id,
                model::ev_escape(&t.url()),
                model::ev_escape(&t.title)
            ));
            self.snapshot_gen += 1;
            changed = true;
            // Phase 4.4.2: the reloaded document is complete — re-apply
            // the preserved scroll position and form state now.
            if t.pending_state_restore {
                t.pending_state_restore = false;
                let js =
                    servo_host::delegate::restore_state_js(t.scroll_est, t.form_state.as_deref());
                if let Some(rt) = self.runtimes.get(&id) {
                    rt.state.pending_js.lock().unwrap().push(js);
                }
                model::emit(format!(
                    "state_restore index={} scroll_est={} forms={}",
                    self.active,
                    t.scroll_est,
                    t.form_state.is_some() as u8
                ));
            }
        } else if complete {
            // Redirects after load: keep the omnibox honest.
            if recordable {
                let u = url.clone().unwrap();
                if t.url() != u {
                    t.history.truncate(t.hindex + 1);
                    t.history.push(u.clone());
                    t.hindex = t.history.len() - 1;
                    model::emit(format!("nav tab={} url={}", id, model::ev_escape(&u)));
                    changed = true;
                }
            }
        }
        if changed {
            self.dirty = true;
            self.sync_title();
        }
        // Phase 4.4.5: keep the tab-search entry fresh for the active tab.
        self.search_index_update(self.active);
        self.sync_background_tabs();
    }

    /// Phase 4.4.5: refresh the search-index entry for strip index `i`
    /// from the shell metadata + the runtime's page snippet.
    fn search_index_update(&mut self, i: usize) {
        let Some(t) = self.tabs.get(i) else { return };
        let Some(id) = t.id else { return };
        let snippet =
            t.id.and_then(|id| self.runtimes.get(&id))
                .and_then(|rt| rt.state.page_snippet.lock().unwrap().clone());
        self.search_index.upsert(id, &t.title, &t.url(), snippet.as_deref());
    }

    /// Phase 2.1: keep shell metadata (URL + title) for BACKGROUND tabs in
    /// sync with their engines, so hibernation restores the right URL even
    /// when the navigation finished while the tab was in the background.
    fn sync_background_tabs(&mut self) {
        let active_id = self.active_id();
        for i in 0..self.tabs.len() {
            let Some(id) = self.tabs[i].id else { continue };
            if Some(id) == active_id {
                continue;
            }
            let (complete, url, title) = match self.runtimes.get(&id) {
                Some(rt) => (
                    rt.state.is_complete(),
                    rt.state.url().map(|u| display_url(&u)),
                    rt.state.title(),
                ),
                None => continue,
            };
            if !complete {
                continue;
            }
            let t = &mut self.tabs[i];
            if let Some(u) = url {
                if u != "about:blank" && t.url() != u {
                    t.history.truncate(t.hindex + 1);
                    t.history.push(u.clone());
                    t.hindex = t.history.len() - 1;
                }
            }
            if let Some(title) = title {
                if !title.is_empty() {
                    t.title = title;
                }
            }
        }
        // Phase 4.4.5: refresh the search entries for background tabs.
        for i in 0..self.tabs.len() {
            if i != self.active {
                self.search_index_update(i);
            }
        }
    }

    // ---- Tab management ---------------------------------------------------

    fn new_tab(&mut self) {
        let ctx = self.offscreen_ctx();
        let (Some(servo), Some(_)) = (self.servo.as_ref(), self.parent_ctx.as_ref()) else {
            return;
        };
        // Phase 2.3: the previously active tab becomes background — throttle it.
        let previous_active = self.active_id();
        let id = self.next_tab_id;
        self.next_tab_id += 1;
        let ctx_dyn: Rc<dyn RenderingContext> = ctx.clone();
        let state = Arc::new(HostState::new());
        let delegate = HostDelegate::new(state.clone(), self.privacy.clone());
        let webview = WebViewBuilder::new(servo, ctx_dyn)
            .delegate(delegate)
            .user_content_manager(self.ucm.clone())
            .build();
        webview.focus();
        webview.resize(PhysicalSize::new(VIEWPORT_W, VIEWPORT_H));
        webview.load(start_page_url());
        self.runtimes.insert(
            id,
            TabRuntime { webview, state, ctx, last_painted_frame: 0, trimmed_at: None },
        );

        let mut tab = UiTab::new();
        tab.id = Some(id);
        tab.status = Status::Loading;
        tab.history.push("brows12://start".to_string());
        self.tabs.push(tab);
        self.active = self.tabs.len() - 1;
        if let Some(prev) = previous_active {
            if let Some(rt) = self.runtimes.get(&prev) {
                rt.webview.set_throttled(true);
            }
        }
        self.dirty = true;
        self.sync_title();
    }

    // ---- Phase 4.4.4: session persistence + restore ----------------------

    /// Startup: restore the last (or a specific) session when one is
    /// requested and loadable; otherwise a cold start with one tab.
    fn restore_session_or_new(&mut self) {
        let mode = std::env::var("BROWS12_SESSION_RESTORE")
            .ok()
            .and_then(|v| servo_host::session::parse_restore_mode(&v));
        let want_restore = mode.is_some();
        let session = mode.and_then(|m| match m {
            servo_host::session::RestoreMode::Last => {
                self.session_path.as_deref().and_then(servo_host::session::load)
            }
            servo_host::session::RestoreMode::Explicit(p) => {
                servo_host::session::load(std::path::Path::new(&p))
            }
        });
        match session {
            Some(sess) => self.restore_session(sess),
            None => {
                self.new_tab();
                if want_restore {
                    model::emit("session_restore_failed cold_start");
                }
            }
        }
        if self.session_path.is_some() {
            let interval = session_save_interval();
            self.next_session_save_at = (interval.as_secs() > 0).then(|| Instant::now() + interval);
        }
    }

    /// Rebuild the strip from a session: groups first (members reference
    /// their ids), then tabs — the ACTIVE tab is rebuilt live, every
    /// other tab comes back as suspended metadata that rehydrates on
    /// activation, so startup builds one webview, not the whole strip.
    fn restore_session(&mut self, sess: servo_host::session::Session) {
        if sess.tabs.is_empty() {
            self.new_tab();
            return;
        }
        for g in &sess.groups {
            let color = servo_host::tabgroups::GroupColor::from_name(&g.color)
                .unwrap_or(servo_host::tabgroups::GroupColor::Grey);
            self.groups.restore_group(g.id, g.name.clone(), color, g.collapsed);
        }
        let active_idx = sess.active.min(sess.tabs.len() - 1);
        for (i, st) in sess.tabs.iter().enumerate() {
            let id = self.next_tab_id;
            self.next_tab_id += 1;
            let mut tab = UiTab::new();
            tab.id = Some(id);
            tab.history = st.history.clone();
            tab.hindex = st.hindex.min(tab.history.len().saturating_sub(1));
            tab.title = st.title.clone();
            tab.scroll_est = st.scroll_est;
            tab.form_state = st.form_state.clone();
            tab.pinned = st.pinned;
            tab.activations = 1;
            if let Some(gid) = st.group {
                self.groups.add_tab(id, gid);
            }
            if i == active_idx {
                tab.status = Status::Loading;
                tab.pending_state_restore = tab.scroll_est > 0.0 || tab.form_state.is_some();
                let url = servo_host::session::Session::tab_url(st);
                let load_url = if url == "brows12://start" || url.is_empty() {
                    start_page_url()
                } else {
                    Url::parse(&url).unwrap_or_else(|_| start_page_url())
                };
                self.tabs.push(tab);
                let runtime = self.spawn_webview(load_url);
                self.runtimes.insert(id, runtime);
                model::emit(format!(
                    "session_live_tab index={i} url={} scroll={} forms={}",
                    model::ev_escape(&url),
                    self.tabs[i].scroll_est,
                    self.tabs[i].form_state.is_some() as u8,
                ));
            } else {
                tab.suspended = true;
                self.tabs.push(tab);
            }
        }
        self.active = active_idx;
        model::emit(format!(
            "session_restored tabs={} groups={} active={} version={}",
            self.tabs.len(),
            sess.groups.len(),
            active_idx,
            sess.version
        ));
        self.dirty = true;
        self.sync_title();
    }

    /// Build one offscreen webview (shared by new_tab, restore and the
    /// session's live tab).
    fn spawn_webview(&mut self, load_url: Url) -> TabRuntime {
        let ctx = self.offscreen_ctx();
        let (Some(servo), Some(_)) = (self.servo.as_ref(), self.parent_ctx.as_ref()) else {
            panic!("servo not initialized");
        };
        let ctx_dyn: Rc<dyn RenderingContext> = ctx.clone();
        let state = Arc::new(HostState::new());
        let delegate = HostDelegate::new(state.clone(), self.privacy.clone());
        let webview = WebViewBuilder::new(servo, ctx_dyn)
            .delegate(delegate)
            .user_content_manager(self.ucm.clone())
            .build();
        webview.focus();
        webview.resize(PhysicalSize::new(VIEWPORT_W, VIEWPORT_H));
        webview.load(load_url);
        TabRuntime { webview, state, ctx, last_painted_frame: 0, trimmed_at: None }
    }

    /// v2.1 Phase 2 — rendering context for a new webview. **Shared by
    /// default**: one context serves every tab (the headless
    /// harness's arrangement, proven across the 31/32-site suite).
    /// Measured on the product shell (4× example.com): per-tab contexts
    /// carried ~115 MB/tab of live allocator heap (GL/llvmpipe
    /// per-context bookkeeping invisible to malloc-size-of) — 580 MB →
    /// 349 MB RSS with the shared context, per-tab marginal 129 → 14 MB.
    /// `BROWS12_SHARED_CTX=0` restores the per-tab arrangement for A/B.
    ///
    /// v2.1 Phase 4 GPU support: on the software lane (`--software` or the
    /// automatic fallback) every webview shares the one software context —
    /// per-tab software contexts would multiply WARP/llvmpipe state.
    fn offscreen_ctx(&mut self) -> Rc<dyn RenderingContext> {
        let per_tab = std::env::var_os("BROWS12_SHARED_CTX").is_some_and(|v| v == "0")
            && self.parent_ctx.as_ref().is_some_and(|p| matches!(p, ParentCtx::Window(_)));
        if !per_tab {
            if let Some(ctx) = self.shared_ctx.as_ref() {
                return ctx.clone();
            }
        }
        let ctx: Rc<dyn RenderingContext> = match self.parent_ctx.as_ref() {
            Some(ParentCtx::Window(parent)) => {
                Rc::new(parent.offscreen_context(PhysicalSize::new(VIEWPORT_W, VIEWPORT_H)))
            }
            Some(ParentCtx::Software(soft)) => soft.clone(),
            None => panic!("no parent rendering context"),
        };
        if !per_tab {
            self.shared_ctx = Some(ctx.clone());
        }
        ctx
    }

    /// Snapshot the strip into a Session and write it atomically.
    fn save_session(&mut self, why: &str) {
        let Some(path) = self.session_path.clone() else { return };
        let active = self.active.min(self.tabs.len().saturating_sub(1));
        let tabs = self
            .tabs
            .iter()
            .map(|t| servo_host::session::SessionTab {
                history: t.history.clone(),
                hindex: t.hindex,
                title: t.title.clone(),
                scroll_est: t.scroll_est,
                form_state: t.form_state.clone(),
                pinned: t.pinned,
                group: t.id.and_then(|id| self.groups.group_of(id)),
            })
            .collect();
        let groups = self
            .groups
            .groups()
            .iter()
            .map(|g| servo_host::session::SessionGroup {
                id: g.id,
                name: g.name.clone(),
                color: g.color.name().to_string(),
                collapsed: g.collapsed,
            })
            .collect();
        let saved_at_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let session = servo_host::session::Session {
            version: servo_host::session::SESSION_VERSION,
            saved_at_ms,
            active,
            tabs,
            groups,
        };
        let ok = servo_host::session::save(&path, &session).is_ok();
        model::emit(format!(
            "session_saved why={why} ok={ok} tabs={} groups={} path={}",
            session.tabs.len(),
            session.groups.len(),
            model::ev_escape(&path.to_string_lossy())
        ));
        // Reschedule the periodic save.
        let interval = session_save_interval();
        self.next_session_save_at = (interval.as_secs() > 0).then(|| Instant::now() + interval);
    }

    /// Central tab activation: restores the tab if hibernated, throttles
    /// the tab being left, unthrottles + focuses the one becoming active.
    fn switch_to(&mut self, index: usize) {
        if index >= self.tabs.len() {
            return;
        }
        if index == self.active {
            return;
        }
        if self.tabs[index].suspended {
            self.restore(index);
        }
        let previous_active = self.active_id();
        // Phase 4.4.2: capture the outgoing tab's form state before it
        // goes to the background — the snapshot is what a later discard
        // preserves (the engine has no synchronous DOM access at
        // hibernation time, so capture happens at the last live moment).
        if let Some(prev) = previous_active {
            if let Some(rt) = self.runtimes.get(&prev) {
                let state = rt.state.clone();
                rt.webview.evaluate_javascript(
                    servo_host::delegate::SERIALIZE_FORMS_JS,
                    move |result| {
                        if let Ok(servo::JSValue::String(json)) = result {
                            *state.form_snapshot.lock().unwrap() = Some(json);
                        }
                    },
                );
            }
        }
        self.active = index;
        self.tabs[index].last_active = Instant::now();
        // Phase 4.4.1: activation recording for predictive hibernation.
        self.tabs[index].activations = self.tabs[index].activations.saturating_add(1);
        let new_active = self.active_id();
        if let Some(prev) = previous_active {
            if Some(prev) != new_active {
                if let Some(rt) = self.runtimes.get(&prev) {
                    rt.webview.set_throttled(true);
                }
            }
        }
        if let Some(id) = new_active {
            if let Some(rt) = self.runtimes.get_mut(&id) {
                rt.webview.set_throttled(false);
                // Phase 4.3.1: undo a budget trim (hidden tab) — the
                // active tab must always be visible and unthrottled.
                rt.webview.show();
                rt.trimmed_at = None;
                rt.webview.focus();
                rt.last_painted_frame = 0; // force a repaint of this tab
            }
        }
        model::emit(format!(
            "switch index={index} url={}",
            model::ev_escape(&self.tabs[index].url())
        ));
        self.dirty = true;
        self.sync_title();
    }

    /// Phase 2.1 — hibernate a background tab: throttle + hide + drop the
    /// WebView (Drop sends CloseWebView; the constellation tears down the
    /// document pipeline and frees its memory). Shell metadata survives so
    /// the tab strip keeps title/URL and activation reloads the page.
    ///
    /// Phase 4.4.2 — `discarded` marks governor-driven hibernations
    /// (memory-aware discard): the strip shows a discard marker and a
    /// `tab_discarded` event notifies the (future) UI + automation. The
    /// tab's scroll estimate and background-captured form snapshot are
    /// preserved in the shell metadata and re-applied on restore.
    fn hibernate(&mut self, index: usize, discarded: bool) {
        if index >= self.tabs.len() || index == self.active {
            return;
        }
        let Some(id) = self.tabs[index].id else { return };
        if self.tabs[index].suspended {
            return;
        }
        let rss_before = servo_host::metrics::rss_kb().unwrap_or(0);
        if let Some(rt) = self.runtimes.remove(&id) {
            let weight = rt.state.page_requests();
            rt.webview.set_throttled(true);
            rt.webview.hide();
            // Phase 4.4.2: harvest the form snapshot captured when the
            // tab went to the background, then drop the pipeline.
            let form = rt.state.form_snapshot.lock().unwrap().clone();
            self.tabs[index].form_state = form;
            // rt (WebView + offscreen ctx) dropped here.
            model::emit(format!("hibernate_weight tab={id} page_requests={weight}"));
        }
        self.tabs[index].suspended = true;
        self.tabs[index].status = Status::Idle;
        self.tabs[index].discarded = discarded;
        if discarded {
            model::emit(format!(
                "tab_discarded index={index} scroll_est={} forms={} url={}",
                self.tabs[index].scroll_est,
                self.tabs[index].form_state.is_some() as u8,
                model::ev_escape(&self.tabs[index].url()),
            ));
        }
        // Phase 4.3.2: return the dropped pipeline's freed arena pages to
        // the OS on EVERY hibernation (the governor loop used to be the
        // only trim site — command-driven hibernation left hundreds of MB
        // allocator-retained, invisible to /proc as free).
        malloc_trim_os();
        model::emit(format!(
            "hibernate tab={} index={index} rss_before_kb={rss_before} url={}",
            id,
            model::ev_escape(&self.tabs[index].url()),
        ));
        self.dirty = true;
    }

    /// Rebuild the WebView of a hibernated tab and reload its URL.
    fn restore(&mut self, index: usize) {
        if index >= self.tabs.len() {
            return;
        }
        if !self.tabs[index].suspended {
            return;
        }
        let ctx = self.offscreen_ctx();
        let (Some(servo), Some(_)) = (self.servo.as_ref(), self.parent_ctx.as_ref()) else {
            return;
        };
        let Some(id) = self.tabs[index].id else { return };
        let url = self.tabs[index].url();
        let load_url = if url == "brows12://start" || url.is_empty() {
            start_page_url()
        } else {
            Url::parse(&url).unwrap_or_else(|_| start_page_url())
        };
        let ctx_dyn: Rc<dyn RenderingContext> = ctx.clone();
        let state = Arc::new(HostState::new());
        let delegate = HostDelegate::new(state.clone(), self.privacy.clone());
        let webview = WebViewBuilder::new(servo, ctx_dyn)
            .delegate(delegate)
            .user_content_manager(self.ucm.clone())
            .build();
        webview.resize(PhysicalSize::new(VIEWPORT_W, VIEWPORT_H));
        webview.load(load_url);
        self.runtimes.insert(
            id,
            TabRuntime { webview, state, ctx, last_painted_frame: 0, trimmed_at: None },
        );
        self.tabs[index].suspended = false;
        self.tabs[index].status = Status::Loading;
        // Phase 4.4.2: re-apply the preserved scroll position (and form
        // state) once the reload completes.
        self.tabs[index].pending_state_restore =
            self.tabs[index].scroll_est > 0.0 || self.tabs[index].form_state.is_some();
        model::emit(format!(
            "restore tab={} index={index} url={} pending_state={}",
            id,
            model::ev_escape(&url),
            self.tabs[index].pending_state_restore as u8,
        ));
        self.dirty = true;
    }

    /// Phase 2.1 + 3.9 — the governor. At `Elevated` RSS (80-99% of
    /// budget) only HEAVY background tabs are reclaimed; at `Critical`
    /// every tab past its (weight-dependent) delay is. Heavy = the page
    /// issued many resource requests (~100MB-class sites), and heavy tabs
    /// become eligible far sooner (`BROWS12_HEAVY_SUSPEND_SECS`).
    ///
    /// Phase 4.3.1 — on top of the global-RSS reclaim, every background
    /// tab is now also checked against its own budget: estimated weight
    /// vs share. Over budget → `TrimCaches` (hide + throttle +
    /// `malloc_trim`); still over after the trim grace → hibernate.
    ///
    /// Phase 4.3.6 — PSI (kernel/cgroup memory-pressure stall info) can
    /// trigger the same reclaim when the *system* is thrashing even if
    /// our own RSS is nominal, on a cooldown.
    ///
    /// Phase 4.3.4 — the JS-heap tier tracks the worst current condition
    /// and is applied engine-wide via `Servo::set_preference` (takes
    /// effect for runtimes created from then on — i.e. every tab
    /// restored from hibernation under pressure comes back smaller).
    fn governor_tick(&mut self) {
        let Some(rss) = servo_host::metrics::rss_kb() else { return };
        let available = servo_host::metrics::mem_available_kb();
        let (psi_snap, psi_source) = match psi::read_psi() {
            Some((s, src)) => (Some(s), src),
            None => (None, "none"),
        };
        let psi_lvl = psi_snap.map(|s| psi::psi_level(&s, &self.psi)).unwrap_or(Pressure::Nominal);
        let rss_level = memory::pressure(&self.governor, rss);
        // The worse of "our own budget" and "the system is thrashing".
        let level = match (rss_level, psi_lvl) {
            (Pressure::Critical, _) | (_, Pressure::Critical) => Pressure::Critical,
            (Pressure::Elevated, _) | (_, Pressure::Elevated) => Pressure::Elevated,
            _ => Pressure::Nominal,
        };

        // ---- Phase 4.4.7: idle-suspend policy pass --------------------
        // Runs at EVERY pressure level (this is the "suspend after N
        // minutes" behavior; the pressure ladders below are additive).
        let suspended = self.idle_suspend_pass();
        if suspended > 0 {
            model::emit(format!("idle_suspend_pass count={suspended}"));
        }

        // ---- Phase 4.3.4: JS-heap tier --------------------------------
        let total = budget::total_budget_kb(available, &self.budget);
        let rss_ratio = rss as f64 / total.max(1) as f64;
        let over_budget = self.over_budget_background_tabs(total);
        let tier = JsHeapTier::for_conditions(rss_ratio, psi_lvl != Pressure::Nominal, over_budget);
        if tier != self.js_tier {
            if let Some(servo) = self.servo.as_ref() {
                servo.set_preference(
                    "js_mem_max",
                    servo::prefs::PrefValue::Int(tier.mem_max_mb() as i64),
                );
                model::emit(format!(
                    "js_heap_tier tier={tier:?} mem_max_mb={mem_max} rss_ratio={rss_ratio:.2} psi_hot={psi_hot} over_budget_tabs={over_budget}",
                    mem_max = tier.mem_max_mb(),
                    psi_hot = psi_lvl != Pressure::Nominal,
                ));
                self.js_tier = tier;
            }
        }

        // ---- PSI-triggered reclaim (4.3.6) ----------------------------
        // When the system (or our cgroup) stalls on memory we act even if
        // our own RSS is nominal — but only once per cooldown, and only
        // on tabs that are past their (weight-aware) suspend delay.
        if psi_lvl != Pressure::Nominal && Instant::now() >= self.next_psi_reclaim_at {
            let reclaimed = self.reclaim_background_tabs(level, rss, "psi");
            self.next_psi_reclaim_at = Instant::now() + self.psi.cooldown;
            if reclaimed > 0 {
                model::emit(format!(
                    "psi_reclaim source={psi_source} level={level:?} reclaimed={reclaimed}"
                ));
            }
        }

        if level == Pressure::Nominal {
            // Even at nominal pressure, enforce per-tab budgets: one
            // runaway tab must not sit under the total while starving
            // the rest (its trim/hibernate decision is independent).
            self.enforce_per_tab_budgets(total);
            return;
        }

        // ---- Global reclaim (Phase 2.1/3.9 path, unchanged) -----------
        let reclaimed = self.reclaim_background_tabs(level, rss, "rss");
        if reclaimed > 0 {
            model::emit(format!(
                "governor_global level={level:?} reclaimed={reclaimed} hibernated_total={}",
                self.governor_hibernated
            ));
        }
        // And the per-tab ladder always runs when the tab strip is live.
        self.enforce_per_tab_budgets(total);
    }

    /// Phase 4.4.7: the idle-suspension pass. Background tabs past the
    /// policy's idle threshold are hibernated, honoring every exemption
    /// (active / pinned / playing media / URL list). `never` disables
    /// ONLY this pass — the pressure responses stay armed.
    fn idle_suspend_pass(&mut self) -> usize {
        let Some(_threshold) = self.suspend_policy.idle_threshold() else {
            return 0;
        };
        let active_id = self.active_id();
        let now = Instant::now();
        let mut targets: Vec<usize> = self
            .tabs
            .iter()
            .enumerate()
            .filter(|(i, t)| {
                let Some(id) = t.id else { return false };
                if t.suspended {
                    return false;
                }
                let media_playing = self
                    .runtimes
                    .get(&id)
                    .map(|rt| rt.state.media_playing.load(std::sync::atomic::Ordering::Relaxed))
                    .unwrap_or(false);
                servo_host::suspend::should_suspend(
                    Some(id) == active_id,
                    t.pinned,
                    media_playing,
                    now.duration_since(t.last_active),
                    &t.url(),
                    &self.suspend_policy,
                    &self.suspend_exempt_urls,
                )
            })
            .map(|(i, _)| i)
            .collect();
        targets.sort_unstable();
        let mut count = 0;
        for i in targets {
            let url = self.tabs[i].url();
            self.hibernate(i, false);
            self.governor_hibernated += 1;
            count += 1;
            model::emit(format!("idle_suspend index={i} url={}", model::ev_escape(&url)));
        }
        count
    }

    /// Phase 4.3.1: how many background tabs are over their individual
    /// budget right now (feeds the JS-heap tier policy).
    fn over_budget_background_tabs(&self, total_kb: u64) -> usize {
        let active_id = self.active_id();
        let weights = self.aligned_tab_weights();
        self.tabs
            .iter()
            .enumerate()
            .filter(|(i, t)| {
                let Some(id) = t.id else { return false };
                if Some(id) == active_id || t.suspended {
                    return false;
                }
                // Phase 4.4.6: pinned tabs never count as over-budget.
                if t.pinned {
                    return false;
                }
                let Some(rt) = self.runtimes.get(&id) else { return false };
                let est = budget::tab_estimate_kb(rt.state.page_requests(), &self.budget);
                let b = budget::tab_budget_kb(total_kb, false, &weights, *i, &self.budget);
                est > b
            })
            .count()
    }

    /// Per-tab weight estimates aligned to `self.tabs` indices (tabs
    /// without a live runtime — suspended or starting — carry the base
    /// weight so index i in this vec always describes tab i).
    fn aligned_tab_weights(&self) -> Vec<u64> {
        self.tabs
            .iter()
            .map(|t| {
                t.id.and_then(|id| self.runtimes.get(&id))
                    .map(|rt| budget::tab_estimate_kb(rt.state.page_requests(), &self.budget))
                    .unwrap_or(self.budget.tab_base_kb)
            })
            .collect()
    }

    /// Phase 4.4.1: per-tab usage signals aligned with the tab strip,
    /// feeding the predictive hibernation order.
    fn aligned_tab_usages(&self) -> Vec<servo_host::tabstats::TabUsage> {
        let now_ms = self.now_ms();
        self.tabs
            .iter()
            .map(|t| {
                let page_requests =
                    t.id.and_then(|id| self.runtimes.get(&id))
                        .map(|rt| rt.state.page_requests())
                        .unwrap_or(0);
                servo_host::tabstats::TabUsage {
                    activations: t.activations,
                    last_active_ms: now_ms
                        .saturating_sub(t.last_active.elapsed().as_millis() as u64),
                    page_requests,
                }
            })
            .collect()
    }

    /// Session-relative monotonic ms clock (deterministic within the
    /// process; only deltas are meaningful).
    fn now_ms(&self) -> u64 {
        self.started.elapsed().as_millis() as u64
    }

    /// The Phase 2.1/3.9 reclaim: hibernate background tabs eligible
    /// under `level` until pressure clears (or candidates run out).
    /// Returns how many tabs were hibernated. `why` tags the event.
    ///
    /// Phase 4.4.1: the ORDER is now predictive — eligible tabs are
    /// suspended least-likely-to-be-returned-to first
    /// (`tabstats::hibernation_order`), with heavier pages first among
    /// near-equal predictions.
    fn reclaim_background_tabs(&mut self, level: Pressure, _rss: u64, why: &str) -> usize {
        let now = Instant::now();
        let active_id = self.active_id();
        let mut candidates: Vec<usize> = self
            .tabs
            .iter()
            .enumerate()
            .filter(|(_, t)| {
                let Some(id) = t.id else { return false };
                if Some(id) == active_id || t.suspended {
                    return false;
                }
                // Phase 4.4.6: pinned tabs NEVER hibernate.
                if t.pinned {
                    return false;
                }
                let Some(rt) = self.runtimes.get(&id) else { return false };
                let weight = rt.state.page_requests();
                // Phase 4.3.4: a background tab with *recent activity*
                // (idle timers / rAF loop still running) is treated on
                // the heavy schedule — it is burning CPU + memory while
                // invisible, so it gives up its pipeline sooner.
                let awake_in_background =
                    matches!(rt.state.ms_since_activity(), Some(ms) if ms < 5_000);
                let effective_weight = if awake_in_background && level != Pressure::Nominal {
                    self.governor.heavy_page_requests.max(weight)
                } else {
                    weight
                };
                memory::eligible_under_pressure(
                    effective_weight,
                    now.duration_since(t.last_active),
                    level,
                    &self.governor,
                )
            })
            .map(|(i, _)| i)
            .collect();
        // Phase 4.4.1: predictive ordering (replaces the old
        // heaviest-first / least-recently-active sort).
        let usages = self.aligned_tab_usages();
        let order = servo_host::tabstats::hibernation_order(&usages, &candidates, self.now_ms());
        candidates.clear();
        candidates.extend(order);
        model::emit(format!("predict_order why={why} level={level:?} candidates={candidates:?}"));
        let mut reclaimed = 0;
        for i in candidates {
            if why != "psi"
                && memory::pressure(&self.governor, servo_host::metrics::rss_kb().unwrap_or(_rss))
                    == Pressure::Nominal
            {
                break;
            }
            self.hibernate(i, true);
            self.governor_hibernated += 1;
            reclaimed += 1;
            // RSS lags the free (allocator retention); sample after a
            // short spin so the loop does not hibernate the whole strip
            // on one stale reading.
            for _ in 0..20 {
                if let Some(s) = self.servo.as_ref() {
                    s.spin_event_loop();
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            // Return freed arena pages to the OS, otherwise RSS keeps the
            // hibernated tabs' memory as allocator-retained pages and the
            // governor's reclaim is invisible to /proc.
            malloc_trim_os();
        }
        reclaimed
    }

    /// Phase 4.3.1: the per-tab graceful-degradation ladder. Background
    /// tabs estimated over their own budget are trimmed first (hide +
    /// throttle + malloc_trim) and hibernated only if still over after
    /// the trim grace. Purely additive to the global reclaim.
    fn enforce_per_tab_budgets(&mut self, total_kb: u64) {
        let active_id = self.active_id();
        let weights = self.aligned_tab_weights();
        // Collect decisions first (borrow checker: hibernate needs &mut).
        let mut actions: Vec<(usize, Degradation)> = Vec::new();
        for (i, t) in self.tabs.iter().enumerate() {
            let Some(id) = t.id else { continue };
            if Some(id) == active_id || t.suspended {
                continue;
            }
            // Phase 4.4.6: pinned tabs are exempt from the ladder.
            if t.pinned {
                continue;
            }
            let Some(rt) = self.runtimes.get(&id) else { continue };
            let est = budget::tab_estimate_kb(rt.state.page_requests(), &self.budget);
            let b = budget::tab_budget_kb(total_kb, false, &weights, i, &self.budget);
            // decide(now, since) computes now - since: pass the trim
            // elapsed as the clock with since = 0, or None when never
            // trimmed (then the ladder starts at TrimCaches).
            let (since, now) = match rt.trimmed_at {
                Some(_) => (Some(0), rt.trimmed_at.unwrap().elapsed().as_millis() as u64),
                None => (None, 0),
            };
            let decision = budget::decide(est, b, since, now, &self.budget);
            if decision != Degradation::None {
                actions.push((i, decision));
            }
        }
        for (i, decision) in actions {
            match decision {
                Degradation::TrimCaches => {
                    if let Some(id) = self.tabs[i].id {
                        if let Some(rt) = self.runtimes.get_mut(&id) {
                            rt.webview.set_throttled(true);
                            rt.webview.hide();
                            rt.trimmed_at = Some(Instant::now());
                        }
                    }
                    malloc_trim_os();
                    model::emit(format!("tab_trim index={i}"));
                }
                Degradation::Hibernate => {
                    self.hibernate(i, true);
                    self.governor_hibernated += 1;
                    model::emit(format!(
                        "tab_budget_hibernate index={i} hibernated_total={}",
                        self.governor_hibernated
                    ));
                }
                Degradation::None => {}
            }
        }
    }

    fn close_tab(&mut self, index: usize) {
        if index < self.tabs.len() {
            let id = self.tabs.remove(index).id;
            if let Some(id) = id {
                // Phase 4.4.3: membership dies with the tab.
                self.groups.tab_closed(id);
                // Phase 4.4.5: drop the search entry too.
                self.search_index.remove(id);
                if let Some(rt) = self.runtimes.remove(&id) {
                    rt.webview.hide();
                }
            }
            if self.tabs.is_empty() {
                self.new_tab();
                return;
            }
            if self.active >= self.tabs.len() {
                self.active = self.tabs.len() - 1;
            }
            // The tab taking the active slot must run unthrottled.
            if let Some(active) = self.active_id() {
                if let Some(rt) = self.runtimes.get_mut(&active) {
                    rt.webview.set_throttled(false);
                    rt.webview.focus();
                    rt.last_painted_frame = 0;
                }
            }
            self.dirty = true;
            self.sync_title();
        }
    }

    // ---- Navigation -------------------------------------------------------

    fn load_in_active(&mut self, normalized: String, record: bool) {
        if normalized.is_empty() {
            return;
        }
        {
            let t = &mut self.tabs[self.active];
            t.status = Status::Loading;
            t.omni_edit = None;
            // Phase 4.4.2: a NEW navigation invalidates the preserved
            // scroll/form state (back/forward reloads of an old entry
            // keep record=false and their state is re-estimated).
            if record {
                t.scroll_est = 0.0;
                t.form_state = None;
                t.pending_state_restore = false;
                t.discarded = false;
                t.history.truncate(t.hindex + 1);
                t.history.push(normalized.clone());
                t.hindex = t.history.len() - 1;
            }
        }
        if let Some(id) = self.active_id() {
            if let Some(rt) = self.runtimes.get(&id) {
                if let Ok(url) = Url::parse(&normalized) {
                    rt.webview.load(url);
                }
            }
        }
        self.dirty = true;
        self.sync_title();
    }

    fn navigate_input(&mut self, raw: String) {
        let normalized = model::normalize_input(&raw);
        self.load_in_active(normalized, true);
    }

    fn go_back(&mut self) {
        let t = &mut self.tabs[self.active];
        if t.can_back() {
            t.hindex -= 1;
            let url = t.url();
            self.load_in_active(url, false);
        }
    }

    fn go_forward(&mut self) {
        let t = &mut self.tabs[self.active];
        if t.can_forward() {
            t.hindex += 1;
            let url = t.url();
            self.load_in_active(url, false);
        }
    }

    fn reload(&mut self) {
        if let Some(id) = self.active_id() {
            if let Some(rt) = self.runtimes.get(&id) {
                rt.webview.reload();
            }
        }
        let t = &mut self.tabs[self.active];
        t.status = Status::Loading;
        t.omni_edit = None;
        self.dirty = true;
        self.sync_title();
    }

    // ---- Input ------------------------------------------------------------

    fn viewport_point(&self) -> (f32, f32) {
        (self.cursor.0, (self.cursor.1 - chrome::CHROME_H).max(0.0))
    }

    fn forward_mouse_move(&mut self) {
        let (x, y) = self.viewport_point();
        if let Some(id) = self.active_id() {
            if let Some(rt) = self.runtimes.get(&id) {
                rt.webview.notify_input_event(InputEvent::MouseMove(MouseMoveEvent::new(
                    DevicePoint::new(x, y).into(),
                )));
            }
        }
    }

    fn forward_mouse_button(&mut self, action: MouseButtonAction) {
        let (x, y) = self.viewport_point();
        if let Some(id) = self.active_id() {
            if let Some(rt) = self.runtimes.get(&id) {
                rt.webview.notify_input_event(InputEvent::MouseButton(MouseButtonEvent::new(
                    action,
                    servo::MouseButton::Primary,
                    DevicePoint::new(x, y).into(),
                )));
            }
        }
    }

    fn forward_wheel(&mut self, dx: f64, dy: f64, mode: WheelMode) {
        let (x, y) = self.viewport_point();
        // Phase 4.4.2: track the active tab's vertical scroll estimate
        // (wheel delta sign: negative dy scrolls DOWN). Clamped at 0;
        // `window.scrollTo` clamps the top end on restore.
        if let Some(t) = self.tabs.get_mut(self.active) {
            t.scroll_est = (t.scroll_est - dy as f32).max(0.0);
        }
        if let Some(id) = self.active_id() {
            if let Some(rt) = self.runtimes.get(&id) {
                rt.webview.notify_input_event(InputEvent::Wheel(WheelEvent::new(
                    WheelDelta { x: x as f64, y: dy, z: 0.0, mode },
                    DevicePoint::new(x, y).into(),
                )));
            }
        }
    }

    fn click(&mut self) {
        let hit = chrome::hit_test(self.cursor.0, self.cursor.1, self.tabs.len());
        match hit {
            Hit::Tab(i) => self.switch_to(i as usize),
            Hit::TabClose(i) => self.close_tab(i as usize),
            Hit::NewTab => self.new_tab(),
            Hit::Back => self.go_back(),
            Hit::Forward => self.go_forward(),
            Hit::Reload => self.reload(),
            Hit::Omnibox => {
                self.tabs[self.active].omni_edit = Some(String::new());
                self.dirty = true;
            }
            Hit::None => {}
        }
    }

    fn scroll(&mut self, dy: f32) {
        self.forward_wheel(0.0, -(dy as f64), WheelMode::DeltaPixel);
    }

    fn key(&mut self, logical: Key, text: Option<String>) {
        let editing = self.tabs[self.active].omni_edit.is_some();
        if editing {
            match &logical {
                Key::Named(NamedKey::Enter) => {
                    let content = self.tabs[self.active].omni_edit.clone().unwrap_or_default();
                    self.navigate_input(content);
                }
                Key::Named(NamedKey::Escape) => {
                    self.tabs[self.active].omni_edit = None;
                    self.dirty = true;
                }
                Key::Named(NamedKey::Backspace) => {
                    let t = &mut self.tabs[self.active];
                    if let Some(e) = t.omni_edit.as_mut() {
                        e.pop();
                    }
                    self.dirty = true;
                }
                _ => {
                    if let Some(txt) = text {
                        let t = &mut self.tabs[self.active];
                        if let Some(e) = t.omni_edit.as_mut() {
                            e.push_str(&txt);
                        }
                        self.dirty = true;
                    }
                }
            }
            return;
        }
        if matches!(&logical, Key::Named(NamedKey::F5)) {
            self.reload();
            return;
        }
        if let Some(id) = self.active_id() {
            if let Some(rt) = self.runtimes.get(&id) {
                if let Some(kb) = winit_to_keyboard_types(&logical, text.as_deref()) {
                    rt.webview
                        .notify_input_event(InputEvent::Keyboard(ServoKeyboardEvent::new(kb)));
                }
            }
        }
    }

    // ---- Automation ---------------------------------------------------------

    fn inject(&mut self, cmd: InjectCmd) {
        match cmd {
            InjectCmd::Omni(text) => {
                self.tabs[self.active].omni_edit = Some(text.clone());
                model::emit(format!("omni text={}", model::ev_escape(&text)));
                self.dirty = true;
            }
            InjectCmd::Text(text) => {
                let t = &mut self.tabs[self.active];
                if t.omni_edit.is_none() {
                    t.omni_edit = Some(String::new());
                }
                if let Some(e) = t.omni_edit.as_mut() {
                    e.push_str(&text);
                }
                if let Some(e) = t.omni_edit.clone() {
                    model::emit(format!("omni text={}", model::ev_escape(&e)));
                }
                self.dirty = true;
            }
            InjectCmd::Return => {
                let content = self.tabs[self.active].omni_edit.clone().unwrap_or_default();
                model::emit(format!("return input={}", model::ev_escape(&content)));
                self.navigate_input(content);
            }
            InjectCmd::Back => self.go_back(),
            InjectCmd::Forward => self.go_forward(),
            InjectCmd::Reload => self.reload(),
            InjectCmd::NewTab => self.new_tab(),
            InjectCmd::Switch(i) => self.switch_to(i),
            InjectCmd::Hibernate(i) => self.hibernate(i, false),
            InjectCmd::Restore(i) => self.restore(i),
            InjectCmd::Scroll(dy) => self.scroll(dy),
            InjectCmd::GroupNew(name, color) => {
                let parsed = servo_host::tabgroups::GroupColor::from_name(&color)
                    .unwrap_or(servo_host::tabgroups::GroupColor::Grey);
                let id = self.groups.create(&name, parsed);
                let g = self.groups.get(id).unwrap();
                model::emit(format!(
                    "group_created id={} name={} color={}",
                    id,
                    model::ev_escape(&g.name),
                    g.color.name()
                ));
                self.dirty = true;
            }
            InjectCmd::GroupDel(id) => {
                let gone = self.groups.delete(id);
                model::emit(format!("group_deleted id={id} ok={gone}"));
                self.dirty = true;
            }
            InjectCmd::GroupAdd(gid, idx) => {
                let ok = match self.tabs.get(idx).and_then(|t| t.id) {
                    Some(tid) => self.groups.add_tab(tid, gid),
                    None => false,
                };
                model::emit(format!("group_add group={gid} tab={idx} ok={ok}"));
                self.dirty = true;
            }
            InjectCmd::GroupRemove(idx) => {
                if let Some(tid) = self.tabs.get(idx).and_then(|t| t.id) {
                    self.groups.remove_tab(tid);
                }
                model::emit(format!("group_remove tab={idx}"));
                self.dirty = true;
            }
            InjectCmd::GroupToggle(gid) => {
                let now = self.groups.toggle_collapsed(gid);
                model::emit(format!(
                    "group_toggle id={gid} collapsed={}",
                    now.map(|b| b as u8).unwrap_or(2)
                ));
                self.dirty = true;
            }
            InjectCmd::GroupsDump => {
                model::emit(format!("groups_json {}", model::ev_escape(&self.groups.to_json())));
            }
            InjectCmd::TabSearch(q) => {
                // Phase 4.4.5: ranked hits, strip index + score per hit.
                let hits = self.search_index.search(&q, 10);
                model::emit(format!(
                    "tabsearch_results query={} count={}",
                    model::ev_escape(&q),
                    hits.len()
                ));
                for (rank, (tab_id, score)) in hits.iter().enumerate() {
                    let idx = self.tabs.iter().position(|t| t.id == Some(*tab_id));
                    if let (Some(i), Some(e)) = (idx, self.search_index.entry(*tab_id)) {
                        model::emit(format!(
                            "tabsearch_hit rank={rank} index={i} score={score:.2} title={} url={} snippet={}",
                            model::ev_escape(&e.title),
                            model::ev_escape(&e.url),
                            model::ev_escape(e.snippet.as_deref().unwrap_or("")),
                        ));
                    }
                }
            }
            InjectCmd::Pin(i) => {
                if let Some(t) = self.tabs.get_mut(i) {
                    t.pinned = !t.pinned;
                    model::emit(format!(
                        "tab_pinned index={i} pinned={} url={}",
                        t.pinned as u8,
                        model::ev_escape(&t.url())
                    ));
                }
                self.dirty = true;
            }
            InjectCmd::Sleep(_) => {}
            InjectCmd::MemReport => {
                // v2.1 Phase 2: attribution — glibc's own live-vs-system
                // split (system − in-use = free-but-resident) to stderr,
                // then the engine report as a `memreport` event.
                if std::env::var_os("BROWS12_MALLOC_STATS").is_some() {
                    eprintln!("brows12-ui: malloc_stats BEGIN");
                    malloc_stats_os();
                    eprintln!("brows12-ui: malloc_stats END");
                }
                self.run_mem_report();
            }
            InjectCmd::Quit => {
                // Phase 4.4.4: persist the session before the loop exits.
                self.save_session("quit");
                model::emit("quit");
                self.quit = true;
            }
        }
    }

    // ---- Presentation -------------------------------------------------------

    fn sync_title(&mut self) {
        let t = &self.tabs[self.active];
        let status = t.status.label();
        let head = if status.is_empty() { t.url() } else { format!("{} — {}", t.url(), status) };
        self.window.set_title(format!("brows12 | {head}").as_str());
    }

    fn draw(&mut self) {
        self.dirty = false;
        let first_present = self.first_present_ms.is_none();
        let start = self.started;

        let size = self.window.inner_size();
        let (w, h) = (size.width.max(1), size.height.max(1));
        let _ = self
            .surface
            .resize(std::num::NonZeroU32::new(w).unwrap(), std::num::NonZeroU32::new(h).unwrap());

        let mut px = Pixmap::new(w, h).expect("framebuffer");
        let hover = self.hover;
        let active = self.active;
        let caret_on = self.caret_on;
        // Phase 4.4.3: per-tab group color for the strip's color bar.
        let group_colors: Vec<Option<[u8; 3]>> = self
            .tabs
            .iter()
            .map(|t| t.id.and_then(|id| self.groups.color_for(id)).map(|c| c.rgb()))
            .collect();
        chrome::draw_chrome(
            &mut px,
            &mut self.text,
            &self.tabs,
            active,
            hover,
            caret_on,
            &group_colors,
        );

        // Paint + read back the active tab's webview, then blit below chrome.
        let mut captured: Option<image::RgbaImage> = None;
        if let (Some(servo), Some(id)) = (self.servo.as_ref(), self.active_id()) {
            if let Some(rt) = self.runtimes.get_mut(&id) {
                servo.spin_event_loop();
                rt.webview.paint();
                let rect = DeviceIntRect::from_size(DeviceIntSize::new(
                    VIEWPORT_W as i32,
                    VIEWPORT_H as i32,
                ));
                captured = rt.ctx.read_to_image(rect);
                rt.last_painted_frame = rt.state.frame_count();
            }
        }
        if let Some(img) = captured {
            let page_w = img.width().min(w);
            let page_h = img.height().min(h.saturating_sub(chrome::CHROME_H as u32));
            let mut page = Pixmap::new(page_w, page_h).expect("page pixmap");
            let raw = img.as_raw();
            for (i, dst) in page.pixels_mut().iter_mut().enumerate() {
                let x = (i % page_w as usize) as u32;
                let y = (i / page_w as usize) as u32;
                let si = (y * img.width() + x) as usize * 4;
                *dst =
                    PremultipliedColorU8::from_rgba(raw[si], raw[si + 1], raw[si + 2], raw[si + 3])
                        .unwrap_or(PremultipliedColorU8::TRANSPARENT);
            }
            px.draw_pixmap(
                0,
                chrome::CHROME_H as i32,
                page.as_ref(),
                &tiny_skia::PixmapPaint::default(),
                tiny_skia::Transform::identity(),
                None,
            );
        }

        let mut buffer = self.surface.buffer_mut().expect("buffer");
        for (i, p) in px.pixels().iter().enumerate() {
            let c = p.demultiply();
            buffer[i] = u32::from_be_bytes([0, c.red(), c.green(), c.blue()]);
        }
        buffer.present().expect("present");

        // Phase 2.2: record + report the first presented frame.
        if first_present {
            self.first_present_ms = Some(start.elapsed().as_millis());
            if let Ok(path) = std::env::var("BROWS12_UI_START_METRICS") {
                let report = serde_json::json!({
                    "servo_build_ms": self.servo_build_ms,
                    "first_present_ms": self.first_present_ms,
                });
                if let Some(parent) = std::path::Path::new(&path).parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                let _ = std::fs::write(&path, report.to_string());
            }
        }

        // Validation hook: dump the composited frame after each page load.
        if let Ok(snapshot_path) = std::env::var("BROWS12_UI_SNAPSHOT") {
            let gen = self.snapshot_gen;
            static LAST_SAVED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            if gen > 0 && LAST_SAVED.load(std::sync::atomic::Ordering::Relaxed) != gen {
                LAST_SAVED.store(gen, std::sync::atomic::Ordering::Relaxed);
                let _ = px.save_png(&snapshot_path);
                model::emit(format!("snapshot path={}", model::ev_escape(&snapshot_path)));
            }
        }
    }
}

/// winit key → keyboard_types event for the page.
fn winit_to_keyboard_types(logical: &Key, _text: Option<&str>) -> Option<KeyboardEvent> {
    let key: KKey = match logical {
        Key::Character(c) => KKey::Character(c.to_string()),
        Key::Named(n) => KKey::Named(match n {
            NamedKey::Enter => keyboard_types::NamedKey::Enter,
            NamedKey::Backspace => keyboard_types::NamedKey::Backspace,
            NamedKey::Escape => keyboard_types::NamedKey::Escape,
            NamedKey::Tab => keyboard_types::NamedKey::Tab,
            NamedKey::ArrowUp => keyboard_types::NamedKey::ArrowUp,
            NamedKey::ArrowDown => keyboard_types::NamedKey::ArrowDown,
            NamedKey::ArrowLeft => keyboard_types::NamedKey::ArrowLeft,
            NamedKey::ArrowRight => keyboard_types::NamedKey::ArrowRight,
            NamedKey::Home => keyboard_types::NamedKey::Home,
            NamedKey::End => keyboard_types::NamedKey::End,
            NamedKey::PageUp => keyboard_types::NamedKey::PageUp,
            NamedKey::PageDown => keyboard_types::NamedKey::PageDown,
            NamedKey::Delete => keyboard_types::NamedKey::Delete,
            _ => return None,
        }),
        _ => return None,
    };
    let code = match &key {
        KKey::Named(keyboard_types::NamedKey::Enter) => Code::Enter,
        KKey::Named(keyboard_types::NamedKey::Backspace) => Code::Backspace,
        KKey::Named(keyboard_types::NamedKey::Escape) => Code::Escape,
        KKey::Named(keyboard_types::NamedKey::Tab) => Code::Tab,
        KKey::Named(keyboard_types::NamedKey::ArrowUp) => Code::ArrowUp,
        KKey::Named(keyboard_types::NamedKey::ArrowDown) => Code::ArrowDown,
        KKey::Named(keyboard_types::NamedKey::ArrowLeft) => Code::ArrowLeft,
        KKey::Named(keyboard_types::NamedKey::ArrowRight) => Code::ArrowRight,
        _ => Code::Unidentified,
    };
    Some(KeyboardEvent {
        state: KeyState::Down,
        key,
        code,
        location: Location::Standard,
        modifiers: Modifiers::empty(),
        repeat: false,
        is_composing: false,
    })
}
