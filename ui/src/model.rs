//! Browser-shell state model: tabs, per-tab status + history, omnibox
//! input normalization (URL vs search query), and the internal start page.
//!
//! v2: Servo runs on the UI thread; each tab owns a Servo `WebView` whose
//! newest frame is captured straight from its offscreen rendering context.

/// The viewport rect the engine renders into (below tab strip + toolbar).
pub const VIEWPORT_W: u32 = 1280;
pub const VIEWPORT_H: u32 = 728; // window 800 - 32 tab strip - 40 toolbar

/// Status of one tab, driven by engine events and load results.
#[derive(Debug, Clone, PartialEq)]
#[allow(dead_code)]
pub enum Status {
    Idle,
    Loading,
    Loaded,
    Error(String),
}

impl Status {
    pub fn label(&self) -> String {
        match self {
            Status::Idle => String::new(),
            Status::Loading => "Loading…".into(),
            Status::Loaded => "Loaded".into(),
            Status::Error(e) => format!("Error: {e}"),
        }
    }
}

/// Shell metadata for one tab. `id` is the key into the shell's
/// webview runtimes (assigned when the tab is created).
pub struct UiTab {
    pub id: Option<u64>,
    pub title: String,
    pub status: Status,
    /// Session history (URLs), with the current position.
    pub history: Vec<String>,
    pub hindex: usize,
    /// Omnibox text being typed (None = show current URL).
    pub omni_edit: Option<String>,
    /// Hibernated: the WebView was dropped; restoring reloads the URL.
    pub suspended: bool,
    /// When this tab last held the active slot (governor eligibility).
    pub last_active: std::time::Instant,
    /// Phase 4.4.1: how many times this tab became the active tab —
    /// predictive-hibernation signal (often-used tabs come back).
    pub activations: u32,
    /// Phase 4.4.2: the tab was DISCARDED by the memory governor (not
    /// by the user). Drives the strip's discard marker + the
    /// `tab_discarded` notification.
    pub discarded: bool,
    /// Phase 4.4.2: shell-tracked vertical scroll estimate (CSS px,
    /// from forwarded wheel deltas). Preserved across hibernation and
    /// re-applied with `window.scrollTo` after the reload completes.
    pub scroll_est: f32,
    /// Phase 4.4.2: form-state JSON captured when the tab went to the
    /// background (`SERIALIZE_FORMS_JS`); re-applied on restore.
    pub form_state: Option<String>,
    /// Phase 4.4.4 (schema-ready for 4.6): pinned tabs never hibernate
    /// and are restored on every startup.
    pub pinned: bool,
    /// Phase 4.4.2: queue the scroll+form restore once the reloaded
    /// document completes.
    pub pending_state_restore: bool,
}

impl UiTab {
    pub fn new() -> Self {
        UiTab {
            id: None,
            title: String::new(),
            status: Status::Idle,
            history: Vec::new(),
            hindex: 0,
            omni_edit: None,
            suspended: false,
            last_active: std::time::Instant::now(),
            activations: 0,
            discarded: false,
            scroll_est: 0.0,
            form_state: None,
            pinned: false,
            pending_state_restore: false,
        }
    }

    pub fn url(&self) -> String {
        self.history.get(self.hindex).cloned().unwrap_or_default()
    }

    pub fn can_back(&self) -> bool {
        self.hindex > 0
    }

    pub fn can_forward(&self) -> bool {
        self.hindex + 1 < self.history.len()
    }
}

/// Commands understood on the `BROWS12_UI_CMD_FIFO` automation channel.
pub enum InjectCmd {
    /// Focus the omnibox and replace its content (like click + type).
    Omni(String),
    /// Enter in the omnibox (navigate to the typed content).
    Return,
    /// Plain text appended to the omnibox when focused.
    Text(String),
    Back,
    Forward,
    Reload,
    NewTab,
    /// Activate tab by index.
    Switch(usize),
    Scroll(f32),
    /// Hibernate a background tab by index (drop its WebView).
    Hibernate(usize),
    /// Restore a hibernated tab by index (rebuild + reload).
    Restore(usize),
    // ---- Phase 4.4.3: tab groups (data layer) ----
    /// Create a group: "name|color" (name may be empty → color name).
    GroupNew(String, String),
    /// Delete a group by id (members are ungrouped, not closed).
    GroupDel(u32),
    /// Add tab (by strip index) to group: "group|tab".
    GroupAdd(u32, usize),
    /// Remove a tab (by strip index) from its group.
    GroupRemove(usize),
    /// Toggle a group's collapsed state.
    GroupToggle(u32),
    /// Dump the group store as a `groups_json` event.
    GroupsDump,
    /// Phase 4.4.5: search open tabs (title/url/snippet).
    TabSearch(String),
    /// Pace the automation script (handled on the reader thread, in order).
    Sleep(u64),
    /// Exit the event loop cleanly (acknowledged with an `quit` event).
    Quit,
}

// ---- Event-out channel -------------------------------------------------
//
// `BROWS12_UI_EVENT_FIFO` carries one JSON-ish line per observable UI
// moment (`tab_created`, `nav`, `loaded`, `load_error`, `omni`, `switch`,
// `quit`) so an automation script can WAIT for real state instead of
// sleeping blindly. A dedicated writer thread owns the FIFO handle and
// re-opens it with backoff whenever no reader is attached, so the UI never
// blocks (or crashes) on the validation plumbing.

use std::sync::mpsc::Sender;
use std::sync::OnceLock;

static EVENT_TX: OnceLock<Sender<String>> = OnceLock::new();

/// Start the event-writer thread for `path`. Events emitted before a reader
/// attaches are delivered as soon as one opens the FIFO.
pub fn init_event_fifo(path: &str) {
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    let path = path.to_string();
    std::thread::Builder::new()
        .name("event-fifo".into())
        .spawn(move || event_writer_loop(path, rx))
        .expect("spawn event writer");
    let _ = EVENT_TX.set(tx);
}

fn event_writer_loop(path: String, rx: Receiver2) {
    let mut handle: Option<std::fs::File> = None;
    while let Ok(mut line) = rx.recv() {
        line.push('\n');
        loop {
            if handle.is_none() {
                handle = std::fs::OpenOptions::new().write(true).open(&path).ok();
            }
            if let Some(f) = handle.as_mut() {
                match std::io::Write::write_all(f, line.as_bytes()) {
                    Ok(()) => break,
                    // Reader went away; reopen when the next one appears.
                    Err(_) => handle = None,
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }
}

// Alias keeps the signature short without importing io::Read anywhere.
type Receiver2 = std::sync::mpsc::Receiver<String>;

/// Emit one event line (no-op unless `init_event_fifo` ran).
pub fn emit(line: impl AsRef<str>) {
    if let Some(tx) = EVENT_TX.get() {
        let _ = tx.send(line.as_ref().to_string());
    }
}

/// Escape a value for the single-line `key=value` event format.
pub fn ev_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('\n', "\\n").replace('\r', "").replace(' ', "_")
}

/// Normalize omnibox input: URLs keep their form, everything that looks
/// like a search query goes to the default search engine.
pub fn normalize_input(raw: &str) -> String {
    let input = raw.trim();
    if input.is_empty() {
        return String::new();
    }

    let has_scheme = input.contains("://");
    let looks_like_host = {
        let host = input.split('/').next().unwrap_or("").split(':').next().unwrap_or("");
        // "example.com", "localhost:8000", "127.0.0.1" …
        host == "localhost"
            || host.parse::<std::net::IpAddr>().is_ok()
            || (host.contains('.') && !host.contains(' ') && !host.ends_with('.'))
    };

    if has_scheme {
        input.to_string()
    } else if looks_like_host {
        format!("https://{input}")
    } else {
        search_url(input)
    }
}

/// Default search: Bing. (Google/DDG are bot-walled or partial in the
/// current engine; see docs/CAPABILITY_REPORT.md §2.)
pub fn search_url(query: &str) -> String {
    format!("https://www.bing.com/search?q={}", percent_encode(query))
}

/// Minimal RFC 3986 percent-encoding for query values.
pub fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Internal start page rendered for fresh tabs (zero network).
#[allow(dead_code)]
pub const START_URL: &str = "brows12://start";

pub const START_HTML: &str = r#"<!DOCTYPE html>
<html>
<head>
<title>New Tab</title>
<style>
  :root { --ink: #1b2733; --accent: #2b6cb0; }
  body { background-color: #f5f6f8; color: var(--ink); font-family: sans-serif; }
  .wrap { display: flex; flex-direction: column; align-items: center; }
  h1 { font-size: 64px; color: var(--accent); margin-top: 160px; }
  p  { font-size: 20px; color: #5a6b7c; }
  .hint { font-size: 16px; color: #8a99a8; }
</style>
</head>
<body>
  <div class="wrap">
    <h1>brows12</h1>
    <p>Featherweight, privacy-first browser engine.</p>
    <p class="hint">Type a URL or a search query in the address bar above.</p>
  </div>
</body>
</html>
"#;
