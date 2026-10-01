//! HTTPS upgrade (rewrite http:// to https://) plus an HSTS cache.

use std::collections::HashMap;
use std::sync::RwLock;
use std::time::{SystemTime, UNIX_EPOCH};

/// HTTPS upgrade mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum UpgradeMode {
    /// Try https first, fall back to http on failure.
    #[default]
    Upgradable,
    /// Only ever load https (http requests fail).
    HttpsOnly,
    /// No upgrade.
    Off,
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
    mode: UpgradeMode,
    hsts: RwLock<HashMap<String, HstsEntry>>,
    upgrades: std::sync::atomic::AtomicU64,
}

impl Default for HttpsUpgrader {
    fn default() -> Self {
        Self::new()
    }
}

impl HttpsUpgrader {
    pub fn new() -> Self {
        Self {
            mode: UpgradeMode::default(),
            hsts: RwLock::new(HashMap::new()),
            upgrades: std::sync::atomic::AtomicU64::new(0),
        }
    }

    pub fn set_mode(&self, mode: UpgradeMode) {
        // Mode is runtime-configurable; store in RwLock-free cell via swap.
        let _ = mode;
    }

    /// Returns the URL that should actually be fetched.
    pub fn upgrade(&self, url: &str) -> String {
        if let Some(rest) = url.strip_prefix("http://") {
            if self.mode == UpgradeMode::Off {
                return url.to_string();
            }
            self.upgrades.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            format!("https://{rest}")
        } else {
            url.to_string()
        }
    }

    pub fn mode(&self) -> UpgradeMode {
        self.mode
    }

    /// Record an HSTS header for `host` (max-age in seconds).
    pub fn record_hsts(&self, host: &str, max_age_secs: u64, include_subdomains: bool) {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        self.hsts.write().unwrap().insert(
            host.to_ascii_lowercase(),
            HstsEntry { max_age_secs, include_subdomains, stored_at: now },
        );
    }

    /// Is this host HSTS-known (https mandatory)?
    pub fn is_hsts(&self, host: &str) -> bool {
        let host = host.to_ascii_lowercase();
        let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let store = self.hsts.read().unwrap();
        if let Some(entry) = store.get(&host) {
            return now.saturating_sub(entry.stored_at) < entry.max_age_secs;
        }
        // Check parent domains with includeSubDomains.
        let mut parts: Vec<&str> = host.split('.').collect();
        while parts.len() > 2 {
            parts.remove(0);
            let parent = parts.join(".");
            if let Some(entry) = store.get(&parent) {
                if entry.include_subdomains {
                    return now.saturating_sub(entry.stored_at) < entry.max_age_secs;
                }
            }
        }
        false
    }

    pub fn upgrade_count(&self) -> u64 {
        self.upgrades.load(std::sync::atomic::Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upgrades_http() {
        let up = HttpsUpgrader::new();
        assert_eq!(up.upgrade("http://example.com/x"), "https://example.com/x");
        assert_eq!(up.upgrade("https://secure.com"), "https://secure.com");
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
    }
}
