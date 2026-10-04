//! HTTPS upgrade (rewrite http:// to https://) plus an HSTS cache and an
//! embedded HSTS preload set.
//!
//! Modes:
//! * [`UpgradeMode::HttpsOnly`] — default (Phase 4 Area 2.4): http:// is
//!   rewritten to https:// everywhere; plain-HTTP hosts on the exception
//!   list keep working.
//! * [`UpgradeMode::Upgradable`] — rewrite but allow engines to fall back
//!   (Servo currently does not implement transparent fallback; the embedder
//!   shows the engine error page).
//! * [`UpgradeMode::Off`] — no rewriting.
//!
//! Excluded from upgrading regardless of mode: IP-literal hosts (local
//! fixtures / loopback), `localhost`, `.onion`, and hosts on the explicit
//! per-site exception list.

use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::sync::RwLock;
use std::time::{SystemTime, UNIX_EPOCH};

use brows12_storage::registrable_domain;

/// HTTPS upgrade mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum UpgradeMode {
    /// Try https first, fall back to http on failure.
    Upgradable,
    /// Only ever load https (http requests fail).
    #[default]
    HttpsOnly,
    /// No upgrade.
    Off,
}

impl UpgradeMode {
    fn from_u8(v: u8) -> Self {
        match v {
            0 => UpgradeMode::Upgradable,
            2 => UpgradeMode::Off,
            _ => UpgradeMode::HttpsOnly,
        }
    }
    fn to_u8(self) -> u8 {
        match self {
            UpgradeMode::Upgradable => 0,
            UpgradeMode::HttpsOnly => 1,
            UpgradeMode::Off => 2,
        }
    }
}

/// HSTS entries live for `max_age` seconds from `stored_at`.
#[derive(Debug, Clone)]
struct HstsEntry {
    max_age_secs: u64,
    include_subdomains: bool,
    stored_at: u64,
}

/// HTTPS upgrader with HSTS state.
pub struct HttpsUpgrader {
    mode: AtomicU8,
    hsts: RwLock<HashMap<String, HstsEntry>>,
    /// Hosts exempted from HTTPS-Only (user-managed per-site exceptions).
    exceptions: RwLock<HashSet<String>>,
    upgrades: AtomicU64,
    hsts_hits: AtomicU64,
}

use std::collections::HashMap;

impl Default for HttpsUpgrader {
    fn default() -> Self {
        Self::new()
    }
}

impl HttpsUpgrader {
    pub fn new() -> Self {
        Self {
            mode: AtomicU8::new(UpgradeMode::default().to_u8()),
            hsts: RwLock::new(HashMap::new()),
            exceptions: RwLock::new(HashSet::new()),
            upgrades: AtomicU64::new(0),
            hsts_hits: AtomicU64::new(0),
        }
    }

    pub fn set_mode(&self, mode: UpgradeMode) {
        self.mode.store(mode.to_u8(), Ordering::Relaxed);
    }

    pub fn mode(&self) -> UpgradeMode {
        UpgradeMode::from_u8(self.mode.load(Ordering::Relaxed))
    }

    /// Per-site HTTPS-Only exception (registrable domain or host).
    pub fn add_exception(&self, host: &str) {
        self.exceptions.write().unwrap().insert(host.to_ascii_lowercase());
    }

    pub fn remove_exception(&self, host: &str) {
        self.exceptions.write().unwrap().remove(&host.to_ascii_lowercase());
    }

    fn is_excepted(&self, host: &str) -> bool {
        let host = host.to_ascii_lowercase();
        let ex = self.exceptions.read().unwrap();
        if ex.contains(&host) {
            return true;
        }
        // Registrable-domain exceptions cover subdomains.
        let site = registrable_domain(&host);
        site != host && ex.contains(&site)
    }

    /// Returns the URL that should actually be fetched, or `None` when the
    /// request must not proceed (never the case today — HttpsOnly rewrites
    /// rather than blocks; blocking semantics live at the caller).
    pub fn upgrade(&self, url: &str) -> Option<String> {
        if let Some(rest) = url.strip_prefix("http://") {
            if self.mode() == UpgradeMode::Off {
                return Some(url.to_string());
            }
            // Host = up to the first '/', port stripped.
            let authority_end = rest.find('/').unwrap_or(rest.len());
            let authority = &rest[..authority_end];
            let host = authority.split('@').next_back().unwrap_or(authority);
            let host = host.split(':').next().unwrap_or(host);
            let host = host.trim_end_matches('.').to_ascii_lowercase();
            if is_upgrade_exempt(&host) || self.is_excepted(&host) {
                return Some(url.to_string());
            }
            self.upgrades.fetch_add(1, Ordering::Relaxed);
            Some(format!("https://{rest}"))
        } else {
            Some(url.to_string())
        }
    }

    /// Record an HSTS header for `host` (max-age in seconds).
    pub fn record_hsts(&self, host: &str, max_age_secs: u64, include_subdomains: bool) {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        if max_age_secs == 0 {
            self.hsts.write().unwrap().remove(&host.to_ascii_lowercase());
            return;
        }
        self.hsts.write().unwrap().insert(
            host.to_ascii_lowercase(),
            HstsEntry { max_age_secs, include_subdomains, stored_at: now },
        );
    }

    /// Is this host HSTS-known (https mandatory)? Preload set + runtime
    /// cache, with includeSubdomains inheritance.
    pub fn is_hsts(&self, host: &str) -> bool {
        let host = host.trim_end_matches('.').to_ascii_lowercase();
        let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let store = self.hsts.read().unwrap();
        if let Some(entry) = store.get(&host) {
            if now.saturating_sub(entry.stored_at) < entry.max_age_secs {
                self.hsts_hits.fetch_add(1, Ordering::Relaxed);
                return true;
            }
        }
        // Walk parent domains for includeSubDomains entries.
        let mut parts: Vec<&str> = host.split('.').collect();
        while parts.len() > 1 {
            parts.remove(0);
            let parent = parts.join(".");
            if let Some(entry) = store.get(&parent) {
                if entry.include_subdomains
                    && now.saturating_sub(entry.stored_at) < entry.max_age_secs
                {
                    self.hsts_hits.fetch_add(1, Ordering::Relaxed);
                    return true;
                }
            }
        }
        drop(store);
        if PRELOAD.iter().any(|h| *h == host) {
            self.hsts_hits.fetch_add(1, Ordering::Relaxed);
            return true;
        }
        let mut parts: Vec<&str> = host.split('.').collect();
        while parts.len() > 1 {
            parts.remove(0);
            let parent = parts.join(".");
            if PRELOAD_SUBDOMAINS.iter().any(|h| *h == parent) {
                self.hsts_hits.fetch_add(1, Ordering::Relaxed);
                return true;
            }
        }
        false
    }

    pub fn upgrade_count(&self) -> u64 {
        self.upgrades.load(Ordering::Relaxed)
    }

    pub fn hsts_hit_count(&self) -> u64 {
        self.hsts_hits.load(Ordering::Relaxed)
    }
}

/// Hosts never upgraded (loopback fixtures, local IPs, .onion).
fn is_upgrade_exempt(host: &str) -> bool {
    host == "localhost"
        || host.ends_with(".onion")
        || host.ends_with(".test")
        || host.parse::<std::net::IpAddr>().is_ok()
}

/// HSTS preload subset: major domains known to ship `max-age ≥ 1 year,
/// includeSubDomains, preload` in the Chromium preload list (curated;
/// refreshed 2026-10). Full-list integration is a follow-up (the Chromium
/// list is ~300k entries; this ships the top-traffic slice).
pub const PRELOAD: &[&str] = &[
    "google.com",
    "youtube.com",
    "facebook.com",
    "instagram.com",
    "whatsapp.com",
    "x.com",
    "twitter.com",
    "wikipedia.org",
    "github.com",
    "gitlab.com",
    "microsoft.com",
    "office.com",
    "live.com",
    "apple.com",
    "icloud.com",
    "amazon.com",
    "netflix.com",
    "linkedin.com",
    "reddit.com",
    "cloudflare.com",
    "telegram.org",
    "signal.org",
    "proton.me",
    "protonmail.com",
    "tutanota.com",
    "mozilla.org",
    "firefox.com",
    "debian.org",
    "kernel.org",
    "gnu.org",
    "rust-lang.org",
    "crates.io",
    "docs.rs",
    "blogspot.com",
    "wordpress.com",
    "medium.com",
    "stripe.com",
    "paypal.com",
    "binance.com",
    "coinbase.com",
    "dropbox.com",
    "zoom.us",
    "slack.com",
    "notion.so",
    "figma.com",
    "canva.com",
    "openai.com",
    "anthropic.com",
    "huggingface.co",
];

/// Preload entries that cover all subdomains (a superset slice of PRELOAD
/// that ships includeSubDomains — used for parent-domain matching).
pub const PRELOAD_SUBDOMAINS: &[&str] = &[
    "google.com",
    "youtube.com",
    "facebook.com",
    "instagram.com",
    "x.com",
    "wikipedia.org",
    "github.com",
    "microsoft.com",
    "apple.com",
    "reddit.com",
    "cloudflare.com",
    "mozilla.org",
    "debian.org",
    "rust-lang.org",
    "crates.io",
    "stripe.com",
    "openai.com",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upgrades_http_by_default() {
        let up = HttpsUpgrader::new();
        assert_eq!(up.mode(), UpgradeMode::HttpsOnly);
        assert_eq!(up.upgrade("http://example.com/x").as_deref(), Some("https://example.com/x"));
        assert_eq!(up.upgrade_count(), 1);
        assert_eq!(up.upgrade("https://secure.com").as_deref(), Some("https://secure.com"));
    }

    #[test]
    fn local_and_ip_hosts_exempt() {
        let up = HttpsUpgrader::new();
        assert_eq!(
            up.upgrade("http://127.0.0.1:8901/page.html").as_deref(),
            Some("http://127.0.0.1:8901/page.html")
        );
        assert_eq!(up.upgrade("http://localhost/x").as_deref(), Some("http://localhost/x"));
        assert_eq!(up.upgrade("http://fixture.test/x").as_deref(), Some("http://fixture.test/x"));
    }

    #[test]
    fn mode_switch_and_exceptions() {
        let up = HttpsUpgrader::new();
        up.set_mode(UpgradeMode::Off);
        assert_eq!(up.upgrade("http://example.com").as_deref(), Some("http://example.com"));
        up.set_mode(UpgradeMode::HttpsOnly);
        up.add_exception("http-only.example");
        assert_eq!(
            up.upgrade("http://http-only.example/x").as_deref(),
            Some("http://http-only.example/x")
        );
        assert_eq!(
            up.upgrade("http://sub.http-only.example/x").as_deref(),
            Some("http://sub.http-only.example/x"),
            "registrable-domain exception covers subdomains"
        );
        up.remove_exception("http-only.example");
        assert_eq!(
            up.upgrade("http://http-only.example/x").as_deref(),
            Some("https://http-only.example/x")
        );
    }

    #[test]
    fn hsts_with_subdomains() {
        let up = HttpsUpgrader::new();
        up.record_hsts("example.com", 3600, true);
        assert!(up.is_hsts("example.com"));
        assert!(up.is_hsts("sub.example.com"));
        assert!(!up.is_hsts("other.com"));
    }

    #[test]
    fn hsts_expiry() {
        let up = HttpsUpgrader::new();
        up.record_hsts("old.com", 0, false);
        assert!(!up.is_hsts("old.com"));
        // max-age 0 removes entries.
        up.record_hsts("gone.com", 10, false);
        up.record_hsts("gone.com", 0, false);
        assert!(!up.is_hsts("gone.com"));
    }

    #[test]
    fn preload_covers_major_domains_and_subdomains() {
        let up = HttpsUpgrader::new();
        assert!(up.is_hsts("google.com"));
        assert!(up.is_hsts("mail.google.com"), "preload parent with includeSubDomains");
        assert!(up.is_hsts("en.wikipedia.org"));
        assert!(!up.is_hsts("example.com"));
        assert!(up.hsts_hit_count() >= 3);
    }

    #[test]
    fn upgradable_mode_still_rewrites() {
        let up = HttpsUpgrader::new();
        up.set_mode(UpgradeMode::Upgradable);
        assert_eq!(up.upgrade("http://a.com/b").as_deref(), Some("https://a.com/b"));
    }
}
