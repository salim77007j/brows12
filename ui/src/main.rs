//! brows12-ui — a minimal, real browser shell proving the engine
//! end-to-end: window + viewport + omnibox + back/forward/reload + tabs.
//!
//! Architecture: the UI thread (winit + softbuffer + tiny-skia chrome) owns
//! presentation only. All engine calls happen on the engine host thread
//! (`engine_host.rs`); the two halves talk over channels and the UI blits
//! the newest `Frame` snapshot of the active tab.

mod chrome;
mod engine_host;
mod model;

use chrome::Hit;
use model::{InjectCmd, Status, UiCmd, UiMsg, UiTab, VIEWPORT_H, VIEWPORT_W};

use std::collections::HashMap;
use std::sync::mpsc::{channel, Sender};
use std::sync::Arc;
use tiny_skia::{Pixmap, PremultipliedColorU8};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, KeyEvent, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowId};

fn main() {
    let event_loop = EventLoop::new().expect("event loop");
    event_loop.set_control_flow(ControlFlow::Wait);
    let mut app = App::default();
    event_loop.run_app(&mut app).expect("run loop");
}

#[derive(Default)]
struct App {
    inner: Option<Gui>,
}

struct Gui {
    window: Arc<Window>,
    #[allow(dead_code)]
    context: softbuffer::Context<Arc<Window>>,
    surface: softbuffer::Surface<Arc<Window>, Arc<Window>>,
    host_tx: Sender<UiCmd>,
    rx: std::sync::mpsc::Receiver<UiMsg>,
    tabs: Vec<UiTab>,
    active: usize,
    frames: HashMap<u64, brows12_api::Frame>,
    raster: brows12_render::raster::Rasterizer,
    cursor: (f32, f32),
    hover: Hit,
    caret_on: bool,
    dirty: bool,
    /// Set by the automation `<QUIT>` command; `about_to_wait` exits.
    quit: bool,
    /// Blink phase (530 ms ticks) the last paint used, so the caret only
    /// repaints when its visibility actually flips.
    blink_phase: u64,
}

impl ApplicationHandler for App {
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
        // Surface owns its display/window handles; the context is kept for
        // drop ordering (dropped after the surface).
        let surface =
            softbuffer::Surface::new(&context, window.clone()).expect("softbuffer surface");

        // Engine host thread owns the Browser and all tabs.
        let (tx, rx) = channel::<UiMsg>();
        let (cmd_tx, cmd_rx) = channel::<UiCmd>();
        let (tx_fifo, wake_fifo) = (tx.clone(), window.clone());
        {
            let wake = window.clone();
            std::thread::spawn(move || {
                let browser = brows12_api::Browser::builder()
                    .viewport(VIEWPORT_W, VIEWPORT_H)
                    .privacy(|p| {
                        p.block_ads = true;
                        p.https_upgrade = true;
                        p.block_cname_cloaking = true;
                    })
                    .max_live_pages(8)
                    .build();
                engine_host::run(browser, cmd_rx, tx, wake);
            });
        }

        let raster = brows12_render::raster::Rasterizer::new(Arc::new(std::sync::Mutex::new(
            cosmic_text::FontSystem::new(),
        )));

        let mut gui = Gui {
            window,
            context,
            surface,
            host_tx: cmd_tx,
            rx,
            tabs: Vec::new(),
            active: 0,
            frames: HashMap::new(),
            raster,
            cursor: (0.0, 0.0),
            hover: Hit::None,
            caret_on: true,
            dirty: true,
            quit: false,
            blink_phase: 0,
        };
        gui.new_tab();
        self.inner = Some(gui);
        // Automation channels (validation under Xvfb): commands arrive on a
        // named pipe and are routed through the same handlers as real
        // mouse/keyboard input; observable moments are emitted on an event
        // pipe so the driver can wait for real state (loaded/url/title).
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
            let tx = tx_fifo;
            let wake = wake_fifo;
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
                        // Sleep is a pacing order, not UI state: keep the
                        // sequencing here so later commands stay ordered.
                        if let Some(InjectCmd::Sleep(ms)) = cmd {
                            std::thread::sleep(std::time::Duration::from_millis(ms));
                            continue;
                        }
                        if let Some(cmd) = cmd {
                            if tx.send(UiMsg::Inject(cmd)).is_ok() {
                                wake.request_redraw();
                            }
                        }
                    }
                }
            });
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let Some(gui) = self.inner.as_mut() else { return };
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::RedrawRequested => {
                gui.drain_messages();
                gui.draw();
            }
            WindowEvent::CursorMoved { position, .. } => {
                gui.cursor = (position.x as f32, position.y as f32);
                gui.hover = chrome::hit_test(gui.cursor.0, gui.cursor.1, gui.tabs.len());
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } => gui.click(),
            WindowEvent::MouseWheel { delta, .. } => {
                let dy = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y * 64.0,
                    MouseScrollDelta::PixelDelta(p) => p.y as f32,
                };
                gui.scroll(-dy);
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
        gui.drain_messages();
        // Caret blink: while the omnibox is in edit mode, flip the caret on
        // a 530 ms cadence and keep the loop waking at the next flip.
        let editing = gui.tabs.iter().any(|t| t.omni_edit.is_some());
        if editing {
            let phase = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64 / 530)
                .unwrap_or(0);
            if phase != gui.blink_phase {
                gui.blink_phase = phase;
                gui.caret_on = phase % 2 == 0;
                gui.dirty = true;
                gui.window.request_redraw();
            }
            event_loop.set_control_flow(ControlFlow::WaitUntil(
                std::time::Instant::now() + std::time::Duration::from_millis(530),
            ));
        }
        if gui.dirty {
            gui.window.request_redraw();
        }
    }
}

impl Gui {
    // ---- Tab management ------------------------------------------------

    fn new_tab(&mut self) {
        self.tabs.push(UiTab::new());
        self.active = self.tabs.len() - 1;
        let _ = self.host_tx.send(UiCmd::NewTab);
        self.dirty = true;
        self.sync_title();
    }

    fn close_tab(&mut self, index: usize) {
        if index < self.tabs.len() {
            let id = self.tabs.remove(index).id;
            let _ = self.host_tx.send(UiCmd::CloseTab { id });
            if self.tabs.is_empty() {
                self.new_tab();
                return;
            }
            if self.active >= self.tabs.len() {
                self.active = self.tabs.len() - 1;
            }
            self.dirty = true;
            self.sync_title();
        }
    }

    // ---- Navigation ------------------------------------------------------

    /// `record=false` for back/forward/reload (history already holds the URL).
    fn load_in_active(&mut self, normalized: String, record: bool) {
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
        if let Some(id) = self.tabs[self.active].id {
            let _ = self.host_tx.send(UiCmd::Navigate { tab: id, url: normalized });
        }
        self.dirty = true;
        self.sync_title();
    }

    fn navigate_input(&mut self, raw: String) {
        let normalized = model::normalize_input(&raw);
        if !normalized.is_empty() {
            self.load_in_active(normalized, true);
        }
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
        let url = self.tabs[self.active].url();
        if url.is_empty() {
            self.load_in_active(model::START_URL.into(), true);
        } else {
            self.load_in_active(url, false);
        }
    }

    // ---- Input -----------------------------------------------------------

    fn click(&mut self) {
        let hit = chrome::hit_test(self.cursor.0, self.cursor.1, self.tabs.len());
        eprintln!("[click] at {:?} -> {:?};", self.cursor, hit);
        match hit {
            Hit::Tab(i) => {
                self.active = i as usize;
                self.dirty = true;
                self.sync_title();
            }
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
        if let Some(id) = self.tabs[self.active].id {
            let _ = self.host_tx.send(UiCmd::Scroll { tab: id, dy });
        }
    }

    fn key<S: std::fmt::Debug>(&mut self, logical: Key<S>, text: Option<String>) {
        eprintln!("[key] logical={logical:?} text={text:?};");
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
        } else if matches!(logical, Key::Named(NamedKey::F5)) {
            self.reload();
        }
    }

    // ---- Engine messages -------------------------------------------------

    fn drain_messages(&mut self) {
        let mut saw_frame = false;
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                UiMsg::TabCreated { id } => {
                    // Fill the first pending (id-less) tab slot in order.
                    if let Some(slot) = self.tabs.iter_mut().find(|t| t.id.is_none()) {
                        slot.id = Some(id);
                        // The host already navigated this fresh tab to the
                        // start page; seed the session history so the
                        // omnibox/back-forward state has a URL from birth.
                        if slot.history.is_empty() {
                            slot.history.push(model::START_URL.to_string());
                        }
                    }
                    model::emit(format!("tab_created id={id}"));
                    self.dirty = true;
                }
                UiMsg::Frame { tab, frame } => {
                    match frame {
                        Some(f) => {
                            self.frames.insert(tab, f);
                        }
                        None => {
                            self.frames.remove(&tab);
                        }
                    }
                    if self.tabs.get(self.active).and_then(|t| t.id) == Some(tab) {
                        saw_frame = true;
                    }
                }
                UiMsg::Engine(brows12_api::EngineEvent::LoadFinished { tab, title }) => {
                    let url = self
                        .tabs
                        .iter()
                        .find(|t| t.id == Some(tab))
                        .map(|t| model::ev_escape(&t.url()))
                        .unwrap_or_default();
                    model::emit(format!(
                        "loaded tab={tab} url={url} title={}",
                        model::ev_escape(&title)
                    ));
                    if let Some(t) = self.tabs.iter_mut().find(|t| t.id == Some(tab)) {
                        t.title = title;
                        t.status = Status::Loaded;
                    }
                    self.dirty = true;
                }
                UiMsg::Engine(brows12_api::EngineEvent::NavigationCommitted { tab, url }) => {
                    // Keep the history entry we just created in sync with the
                    // URL the engine actually committed (e.g. HTTPS upgrade).
                    if let Some(t) = self.tabs.iter_mut().find(|t| t.id == Some(tab)) {
                        if t.status == Status::Loading && !t.history.is_empty() {
                            t.history[t.hindex] = url.clone();
                        }
                    }
                    model::emit(format!("nav tab={tab} url={}", model::ev_escape(&url)));
                    self.dirty = true;
                }
                UiMsg::Engine(_) => {}
                UiMsg::Inject(cmd) => match cmd {
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
                    InjectCmd::Back => {
                        model::emit(format!(
                            "back from={} index={}",
                            model::ev_escape(&self.tabs[self.active].url()),
                            self.tabs[self.active].hindex
                        ));
                        self.go_back();
                    }
                    InjectCmd::Forward => {
                        model::emit(format!(
                            "forward from={} index={}",
                            model::ev_escape(&self.tabs[self.active].url()),
                            self.tabs[self.active].hindex
                        ));
                        self.go_forward();
                    }
                    InjectCmd::Reload => {
                        model::emit(format!(
                            "reload url={}",
                            model::ev_escape(&self.tabs[self.active].url())
                        ));
                        self.reload();
                    }
                    InjectCmd::NewTab => {
                        model::emit("newtab requested");
                        self.new_tab();
                    }
                    InjectCmd::Switch(i) => {
                        if i < self.tabs.len() {
                            self.active = i;
                            let url = self.tabs[self.active].url();
                            model::emit(format!("switch index={i} url={}", model::ev_escape(&url)));
                            self.dirty = true;
                            self.sync_title();
                        }
                    }
                    InjectCmd::Scroll(dy) => self.scroll(dy),
                    // Sleep never reaches the UI thread: the FIFO reader
                    // paces the script before forwarding later commands.
                    InjectCmd::Sleep(_) => {}
                    InjectCmd::Quit => {
                        model::emit("quit");
                        self.quit = true;
                    }
                },
                UiMsg::LoadResult { tab, result } => {
                    if let Some(t) = self.tabs.iter_mut().find(|t| t.id == Some(tab)) {
                        if let Err(e) = result {
                            model::emit(format!(
                                "load_error tab={tab} error={}",
                                model::ev_escape(&e)
                            ));
                            t.status = Status::Error(e);
                        }
                        // On Ok we still wait for LoadFinished (title).
                    }
                    self.dirty = true;
                }
            }
        }
        if saw_frame {
            self.dirty = true;
        }
    }

    // ---- Presentation ------------------------------------------------------

    fn sync_title(&mut self) {
        let t = &self.tabs[self.active];
        let status = t.status.label();
        let head = if status.is_empty() { t.url() } else { format!("{} — {}", t.url(), status) };
        self.window.set_title(format!("brows12 | {head}").as_str());
    }

    fn draw(&mut self) {
        self.dirty = false;

        let size = self.window.inner_size();
        let (w, h) = (size.width.max(1), size.height.max(1));
        let _ = self
            .surface
            .resize(std::num::NonZeroU32::new(w).unwrap(), std::num::NonZeroU32::new(h).unwrap());

        let mut px = Pixmap::new(w, h).expect("framebuffer");
        chrome::draw_chrome(
            &mut px,
            &mut self.raster,
            &self.tabs,
            self.active,
            self.hover,
            self.caret_on,
        );

        // Blit the active tab's newest frame into the viewport.
        if let Some(frame) = self.tabs[self.active].id.as_ref().and_then(|id| self.frames.get(id)) {
            if frame.width > 0 && frame.height > 0 {
                if let Some(mut page) = Pixmap::new(frame.width, frame.height) {
                    let src = frame.rgba_premultiplied();
                    for (i, dst) in page.pixels_mut().iter_mut().enumerate() {
                        let b = &src[i * 4..i * 4 + 4];
                        *dst = PremultipliedColorU8::from_rgba(b[0], b[1], b[2], b[3])
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
            }
        }

        let mut buffer = self.surface.buffer_mut().expect("buffer");
        for (i, p) in px.pixels().iter().enumerate() {
            let c = p.demultiply();
            buffer[i] = u32::from_be_bytes([0, c.red(), c.green(), c.blue()]);
        }
        buffer.present().expect("present");
    }
}
