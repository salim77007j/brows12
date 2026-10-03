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
use servo::input_events::{
    InputEvent, MouseButtonAction, MouseButtonEvent, MouseMoveEvent,
};
use servo::{
    DeviceIntRect, DeviceIntSize, DevicePoint, KeyboardEvent as ServoKeyboardEvent,
    OffscreenRenderingContext, RenderingContext, Servo, ServoBuilder, UserContentManager,
    WebView, WebViewBuilder, WheelDelta, WheelEvent, WheelMode, WindowRenderingContext,
};
use servo_host::delegate::{HostDelegate, HostState};
use servo_host::memory::{self, GovernorConfig, Pressure};
use servo_host::privacy::PrivacyHost;
use servo_host::waker::HostWakerEvent;
use tiny_skia::{Pixmap, PremultipliedColorU8};
use url::Url;
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::dpi::PhysicalSize;
use winit::event::{ElementState, KeyEvent, MouseButton as WinitButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::{Key, NamedKey};
use winit::raw_window_handle::{HasDisplayHandle, HasWindowHandle};
use winit::window::{Window, WindowId};

use chrome::Hit;
use model::{InjectCmd, Status, UiTab, START_HTML, VIEWPORT_H, VIEWPORT_W};
use text::UiText;

fn main() {
    let started = Instant::now();
    let event_loop = EventLoop::with_user_event()
        .build()
        .expect("event loop");
    event_loop.set_control_flow(ControlFlow::Wait);
    let mut app = App {
        proxy: Some(event_loop.create_proxy()),
        inner: None,
        started,
    };
    init_pending_channel();
    event_loop.run_app(&mut app).expect("run loop");
}

struct App {
    inner: Option<Gui>,
    proxy: Option<EventLoopProxy<HostWakerEvent>>,
    /// Process start, for the startup metrics report.
    started: Instant,
}

/// Per-tab Servo runtime: the webview plus its offscreen context and state.
struct TabRuntime {
    webview: WebView,
    state: Arc<HostState>,
    #[allow(dead_code)]
    ctx: Rc<OffscreenRenderingContext>,
    last_painted_frame: u64,
}

#[allow(dead_code)]
struct Gui {
    window: Arc<Window>,
    context: softbuffer::Context<Arc<Window>>,
    surface: softbuffer::Surface<Arc<Window>, Arc<Window>>,
    proxy: EventLoopProxy<HostWakerEvent>,
    servo: Option<Servo>,
    parent_ctx: Option<Rc<WindowRenderingContext>>,
    ucm: Rc<UserContentManager>,
    privacy: Arc<PrivacyHost>,
    tabs: Vec<UiTab>,
    runtimes: HashMap<u64, TabRuntime>,
    next_tab_id: u64,
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

        let display_handle = window.display_handle().expect("display handle");
        let window_handle = window.window_handle().expect("window handle");
        let size = PhysicalSize::new(chrome::WIN_W, chrome::WIN_H);
        let parent_ctx = Rc::new(
            WindowRenderingContext::new(display_handle, window_handle, size)
                .expect("GL context for window (run under Xvfb or a desktop session)"),
        );

        let proxy = self
            .proxy
            .as_ref()
            .expect("event loop proxy")
            .clone();
        let waker = servo_host::waker::ProxyWaker::new(proxy.clone());
        let build_started = Instant::now();
        let servo: Servo = ServoBuilder::default()
            .event_loop_waker(Box::new(waker))
            .preferences(servo_host::brows12_preferences())
            .build();
        let servo_build_ms = build_started.elapsed().as_millis();

        let privacy = PrivacyHost::new();
        let ucm = Rc::new(UserContentManager::new(&servo));
        privacy.set_user_content_manager(ucm.clone());

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
            active: 0,
            text: UiText::new(),
            cursor: (0.0, 0.0),
            hover: Hit::None,
            caret_on: true,
            dirty: true,
            quit: false,
            blink_phase: 0,
            snapshot_gen: 0,
            governor: GovernorConfig::from_env(),
            next_governor_at: Instant::now() + GovernorConfig::default().interval,
            governor_hibernated: 0,
            servo_build_ms,
            first_present_ms: None,
            started: self.started,
        };
        gui.new_tab();
        let start_proxy = gui.proxy.clone();
        let start_wake = gui.window.clone();
        self.inner = Some(gui);
        // Phase 2.2: present the chrome immediately — do not wait for the
        // first engine frame to drive the first present.
        start_wake.request_redraw();

        // Automation channels (validation under Xvfb) — same protocol as v1.
        if let Ok(fifo) = std::env::var("BROWS12_UI_CMD_FIFO") {
            if let Ok(events) = std::env::var("BROWS12_UI_EVENT_FIFO") {
                model::init_event_fifo(&events);
            }
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
                        } else if let Some(rest) = line.strip_prefix("<SLEEP> ") {
                            rest.parse::<u64>().ok().map(InjectCmd::Sleep)
                        } else {
                            match line.as_str() {
                                "<RETURN>" => Some(InjectCmd::Return),
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
                            let sent = PENDING_TX
                                .get()
                                .map(|tx| tx.send(cmd).is_ok())
                                .unwrap_or(false);
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
            WindowEvent::CloseRequested => event_loop.exit(),
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
        let mut next_wake = if busy {
            Some(Instant::now() + Duration::from_millis(16))
        } else {
            None
        };
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

fn base64_encode(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
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
        self.sync_background_tabs();
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
    }

    // ---- Tab management ---------------------------------------------------

    fn new_tab(&mut self) {
        let (Some(servo), Some(parent)) = (self.servo.as_ref(), self.parent_ctx.as_ref()) else {
            return;
        };
        // Phase 2.3: the previously active tab becomes background — throttle it.
        let previous_active = self.active_id();
        let id = self.next_tab_id;
        self.next_tab_id += 1;
        let ctx = Rc::new(parent.offscreen_context(PhysicalSize::new(VIEWPORT_W, VIEWPORT_H)));
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
            TabRuntime { webview, state, ctx, last_painted_frame: 0 },
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
        self.active = index;
        self.tabs[index].last_active = Instant::now();
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
    fn hibernate(&mut self, index: usize) {
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
            // rt (WebView + offscreen ctx) dropped here.
            model::emit(format!("hibernate_weight tab={id} page_requests={weight}"));
        }
        self.tabs[index].suspended = true;
        self.tabs[index].status = Status::Idle;
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
        let (Some(servo), Some(parent)) = (self.servo.as_ref(), self.parent_ctx.as_ref()) else {
            return;
        };
        let Some(id) = self.tabs[index].id else { return };
        let url = self.tabs[index].url();
        let load_url = if url == "brows12://start" || url.is_empty() {
            start_page_url()
        } else {
            Url::parse(&url).unwrap_or_else(|_| start_page_url())
        };
        let ctx = Rc::new(parent.offscreen_context(PhysicalSize::new(VIEWPORT_W, VIEWPORT_H)));
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
            TabRuntime { webview, state, ctx, last_painted_frame: 0 },
        );
        self.tabs[index].suspended = false;
        self.tabs[index].status = Status::Loading;
        model::emit(format!(
            "restore tab={} index={index} url={}",
            id,
            model::ev_escape(&url),
        ));
        self.dirty = true;
    }

    /// Phase 2.1 + 3.9 — the governor. At `Elevated` RSS (80-99% of
    /// budget) only HEAVY background tabs are reclaimed; at `Critical`
    /// every tab past its (weight-dependent) delay is. Heavy = the page
    /// issued many resource requests (~100MB-class sites), and heavy tabs
    /// become eligible far sooner (`BROWS12_HEAVY_SUSPEND_SECS`).
    fn governor_tick(&mut self) {
        let Some(rss) = servo_host::metrics::rss_kb() else { return };
        let level = memory::pressure(&self.governor, rss);
        if level == Pressure::Nominal {
            return;
        }
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
                let weight = self
                    .runtimes
                    .get(&id)
                    .map(|rt| rt.state.page_requests())
                    .unwrap_or(0);
                memory::eligible_under_pressure(
                    weight,
                    now.duration_since(t.last_active),
                    level,
                    &self.governor,
                )
            })
            .map(|(i, _)| i)
            .collect();
        // Reclaim heaviest first, then least-recently-active.
        candidates.sort_by_key(|&i| {
            let weight = self.tabs[i]
                .id
                .and_then(|id| self.runtimes.get(&id))
                .map(|rt| rt.state.page_requests())
                .unwrap_or(0);
            (std::cmp::Reverse(weight), self.tabs[i].last_active)
        });
        let mut rss_now = rss;
        for i in candidates {
            if memory::pressure(&self.governor, rss_now) == Pressure::Nominal {
                break;
            }
            self.hibernate(i);
            self.governor_hibernated += 1;
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
            unsafe {
                libc::malloc_trim(0);
            }
            rss_now = servo_host::metrics::rss_kb().unwrap_or(rss_now);
        }
        if rss_now != rss {
            model::emit(format!(
                "governor rss_before_kb={rss} rss_after_kb={rss_now} hibernated_total={}",
                self.governor_hibernated
            ));
        }
    }

    fn close_tab(&mut self, index: usize) {
        if index < self.tabs.len() {
            let id = self.tabs.remove(index).id;
            if let Some(id) = id {
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
            if record {
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
        (
            self.cursor.0,
            (self.cursor.1 - chrome::CHROME_H).max(0.0),
        )
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
        if let Some(id) = self.active_id() {
            if let Some(rt) = self.runtimes.get(&id) {
                rt.webview.notify_input_event(InputEvent::Wheel(WheelEvent::new(
                    WheelDelta { x: dx, y: dy, z: 0.0, mode },
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
            InjectCmd::Hibernate(i) => self.hibernate(i),
            InjectCmd::Restore(i) => self.restore(i),
            InjectCmd::Scroll(dy) => self.scroll(dy),
            InjectCmd::Sleep(_) => {}
            InjectCmd::Quit => {
                model::emit("quit");
                self.quit = true;
            }
        }
    }

    // ---- Presentation -------------------------------------------------------

    fn sync_title(&mut self) {
        let t = &self.tabs[self.active];
        let status = t.status.label();
        let head = if status.is_empty() {
            t.url()
        } else {
            format!("{} — {}", t.url(), status)
        };
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
        chrome::draw_chrome(&mut px, &mut self.text, &self.tabs, active, hover, caret_on);

        // Paint + read back the active tab's webview, then blit below chrome.
        let mut captured: Option<image::RgbaImage> = None;
        if let (Some(servo), Some(id)) = (self.servo.as_ref(), self.active_id()) {
            if let Some(rt) = self.runtimes.get_mut(&id) {
                servo.spin_event_loop();
                rt.webview.paint();
                let rect =
                    DeviceIntRect::from_size(DeviceIntSize::new(VIEWPORT_W as i32, VIEWPORT_H as i32));
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
                *dst = PremultipliedColorU8::from_rgba(
                    raw[si],
                    raw[si + 1],
                    raw[si + 2],
                    raw[si + 3],
                )
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
            static LAST_SAVED: std::sync::atomic::AtomicU64 =
                std::sync::atomic::AtomicU64::new(0);
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
