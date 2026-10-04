//! Phase 4 Area 4.4 — full session persistence + restore.
//!
//! A `Session` captures the complete tab-strip state: per-tab history
//! (with current position), title, scroll estimate, form-state JSON
//! (4.2 capture), pinned flag (schema-ready for 4.6), group membership
//! (4.3), plus the active index. Written atomically on quit and
//! periodically; restored at startup — the active tab is rebuilt live,
//! every other tab comes back as suspended metadata that rehydrates on
//! activation (the existing hibernation machinery), so startup cost is
//! one tab, not the whole strip.
//!
//! Restore modes (env, read by the shell):
//! - `BROWS12_SESSION_FILE=<path>`   — where "last session" lives
//! - `BROWS12_SESSION_RESTORE=1|last`— restore the last session
//! - `BROWS12_SESSION_RESTORE=<path>`— restore a specific session file

use std::io::Write;
use std::path::Path;

use serde::{Deserialize, Serialize};

/// Schema version; a loader refuses newer/unknown layouts.
pub const SESSION_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionTab {
    /// Full session history (URLs), `hindex` = current entry.
    pub history: Vec<String>,
    pub hindex: usize,
    pub title: String,
    /// Shell scroll estimate (4.2) — re-applied via `window.scrollTo`.
    pub scroll_est: f32,
    /// Form-state JSON captured on switch-away (4.2) — re-applied.
    #[serde(default)]
    pub form_state: Option<String>,
    /// Phase 4.6 pinned tabs (never hibernated; restored every start).
    #[serde(default)]
    pub pinned: bool,
    /// Group id the tab belongs to (4.3), if any.
    #[serde(default)]
    pub group: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionGroup {
    pub id: u32,
    pub name: String,
    /// GroupColor name ("grey".."cyan").
    pub color: String,
    pub collapsed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub version: u32,
    /// Unix millis of the save.
    pub saved_at_ms: u64,
    /// Index into `tabs` of the tab to restore active.
    pub active: usize,
    pub tabs: Vec<SessionTab>,
    #[serde(default)]
    pub groups: Vec<SessionGroup>,
}

impl Session {
    /// A session with one fresh start tab (what a cold start would show).
    pub fn single_start_tab(now_ms: u64, start_url: &str) -> Session {
        Session {
            version: SESSION_VERSION,
            saved_at_ms: now_ms,
            active: 0,
            tabs: vec![SessionTab {
                history: vec![start_url.to_string()],
                hindex: 0,
                title: String::new(),
                scroll_est: 0.0,
                form_state: None,
                pinned: false,
                group: None,
            }],
            groups: Vec::new(),
        }
    }

    /// The restored tab's current URL (empty history → start page).
    pub fn tab_url(tab: &SessionTab) -> String {
        tab.history.get(tab.hindex).cloned().unwrap_or_default()
    }
}

/// Write `session` atomically (tmp file + rename) so a crash mid-write
/// cannot corrupt the last good session.
pub fn save(path: &Path, session: &Session) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("json.tmp");
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(serde_json::to_string_pretty(session).unwrap_or_default().as_bytes())?;
        f.sync_all().ok();
    }
    std::fs::rename(&tmp, path)
}

/// Load a session file. Returns None for missing, corrupt, or
/// future-version files (restore fails closed, like a cold start).
pub fn load(path: &Path) -> Option<Session> {
    let bytes = std::fs::read(path).ok()?;
    let s: Session = serde_json::from_slice(&bytes).ok()?;
    if s.version > SESSION_VERSION {
        return None;
    }
    if s.tabs.is_empty() {
        return None;
    }
    Some(s)
}

/// Parse `BROWS12_SESSION_RESTORE`: `Some(RestoreMode)`.
#[derive(Debug, Clone, PartialEq)]
pub enum RestoreMode {
    /// Restore the file at `BROWS12_SESSION_FILE`.
    Last,
    /// Restore this specific file.
    Explicit(String),
}

pub fn parse_restore_mode(v: &str) -> Option<RestoreMode> {
    match v {
        "" | "0" | "no" | "off" => None,
        "1" | "last" | "true" => Some(RestoreMode::Last),
        other => Some(RestoreMode::Explicit(other.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Session {
        Session {
            version: SESSION_VERSION,
            saved_at_ms: 1_700_000_000_000,
            active: 1,
            tabs: vec![
                SessionTab {
                    history: vec!["https://a.example/".into(), "https://a.example/x".into()],
                    hindex: 1,
                    title: "A".into(),
                    scroll_est: 240.0,
                    form_state: Some(r#"[["text","hello"]]"#.into()),
                    pinned: false,
                    group: Some(2),
                },
                SessionTab {
                    history: vec!["https://b.example/".into()],
                    hindex: 0,
                    title: "B".into(),
                    scroll_est: 0.0,
                    form_state: None,
                    pinned: true,
                    group: Some(2),
                },
                SessionTab {
                    history: vec!["brows12://start".into()],
                    hindex: 0,
                    title: String::new(),
                    scroll_est: 0.0,
                    form_state: None,
                    pinned: false,
                    group: None,
                },
            ],
            groups: vec![SessionGroup {
                id: 2,
                name: "research".into(),
                color: "purple".into(),
                collapsed: false,
            }],
        }
    }

    #[test]
    fn save_load_roundtrip() {
        let dir = std::env::temp_dir().join(format!("b4-session-{}", std::process::id()));
        let path = dir.join("session.json");
        let s = sample();
        save(&path, &s).unwrap();
        let loaded = load(&path).unwrap();
        assert_eq!(loaded.active, 1);
        assert_eq!(loaded.tabs.len(), 3);
        assert_eq!(loaded.tabs[0].history, vec!["https://a.example/", "https://a.example/x"]);
        assert_eq!(loaded.tabs[0].scroll_est, 240.0);
        assert_eq!(loaded.tabs[1].pinned, true);
        assert_eq!(loaded.tabs[0].group, Some(2));
        assert_eq!(loaded.groups[0].name, "research");
        // Atomic write left no temp file behind.
        assert!(!dir.join("session.json.tmp").exists());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn corrupt_and_version_refuse() {
        let dir = std::env::temp_dir().join(format!("b4-session-bad-{}", std::process::id()));
        std::fs::create_dir_all(&dir).ok();
        let p = dir.join("c.json");
        std::fs::write(&p, b"{not json").unwrap();
        assert!(load(&p).is_none());
        let mut s = sample();
        s.version = SESSION_VERSION + 5;
        save(&dir.join("v.json"), &s).unwrap();
        assert!(load(&dir.join("v.json")).is_none(), "future version refused");
        // Empty tabs refused.
        let mut s2 = sample();
        s2.tabs.clear();
        save(&dir.join("e.json"), &s2).unwrap();
        assert!(load(&dir.join("e.json")).is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn restore_mode_parsing() {
        assert_eq!(parse_restore_mode("1"), Some(RestoreMode::Last));
        assert_eq!(parse_restore_mode("last"), Some(RestoreMode::Last));
        assert_eq!(parse_restore_mode("0"), None);
        assert_eq!(parse_restore_mode(""), None);
        assert_eq!(
            parse_restore_mode("/tmp/other.json"),
            Some(RestoreMode::Explicit("/tmp/other.json".into()))
        );
    }

    #[test]
    fn single_start_tab_shape() {
        let s = Session::single_start_tab(42, "brows12://start");
        assert_eq!(s.tabs.len(), 1);
        assert_eq!(Session::tab_url(&s.tabs[0]), "brows12://start");
        assert_eq!(s.active, 0);
    }
}
