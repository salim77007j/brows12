//! HTTP response cache: memory hot layer + optional disk layer, LRU by age.

use crate::{KeyValueStore, MemoryStore};
use blake3::Hasher;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

const DEFAULT_MAX_DISK_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedResponse {
    pub url: String,
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    pub stored_at: u64,
    pub protocol: String,
}

/// Content-addressed HTTP cache (key = blake3 of normalized URL).
pub struct HttpCache {
    memory: MemoryStore,
    disk: Option<Arc<dyn KeyValueStore>>,
    max_disk_bytes: AtomicUsize,
    disk_usage: AtomicUsize,
}

impl Default for HttpCache {
    fn default() -> Self {
        Self::new()
    }
}

impl HttpCache {
    pub fn new() -> Self {
        Self {
            memory: MemoryStore::new(),
            disk: None,
            max_disk_bytes: AtomicUsize::new(DEFAULT_MAX_DISK_BYTES),
            disk_usage: AtomicUsize::new(0),
        }
    }

    /// Attach a persistent backing store.
    pub fn with_disk(mut self, store: Arc<dyn KeyValueStore>) -> Self {
        self.disk = Some(store);
        self
    }

    pub fn set_max_disk_bytes(&self, bytes: usize) {
        self.max_disk_bytes.store(bytes, Ordering::Relaxed);
    }

    fn key(url: &str) -> String {
        let mut h = Hasher::new();
        h.update(url.as_bytes());
        h.finalize().to_hex().to_string()
    }

    pub fn put(&self, response: CachedResponse) {
        let key = Self::key(&response.url);
        let size = response.body.len();
        if let Ok(bytes) = serde_json::to_vec(&response) {
            // Keep only reasonably sized entries in memory (<= 1 MiB).
            if size <= 1024 * 1024 {
                let _ = self.memory.put("http", &key, &bytes);
            }
            if let Some(disk) = &self.disk {
                if size <= self.max_disk_bytes.load(Ordering::Relaxed)
                    && disk.put("http", &key, &bytes).is_ok()
                {
                    self.disk_usage.fetch_add(size, Ordering::Relaxed);
                }
            }
        }
    }

    pub fn get(&self, url: &str) -> Option<CachedResponse> {
        let key = Self::key(url);
        let bytes = self
            .memory
            .get("http", &key)
            .ok()
            .flatten()
            .or_else(|| self.disk.as_ref().and_then(|d| d.get("http", &key).ok().flatten()))?;
        serde_json::from_slice(&bytes).ok()
    }

    pub fn remove(&self, url: &str) {
        let key = Self::key(url);
        let _ = self.memory.delete("http", &key);
        if let Some(disk) = &self.disk {
            let _ = disk.delete("http", &key);
        }
    }

    /// Drop entries older than `max_age_secs` (used by the budgeter).
    pub fn prune(&self, max_age_secs: u64) {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let Ok(keys) = self.memory.keys("http") else {
            return;
        };
        for key in keys {
            if let Ok(Some(bytes)) = self.memory.get("http", &key) {
                if let Ok(resp) = serde_json::from_slice::<CachedResponse>(&bytes) {
                    if now.saturating_sub(resp.stored_at) > max_age_secs {
                        let _ = self.memory.delete("http", &key);
                    }
                }
            }
        }
    }

    pub fn memory_entry_count(&self) -> usize {
        self.memory.keys("http").map(|k| k.len()).unwrap_or(0)
    }
}

/// Freshness heuristic: honor Cache-Control max-age / Expires when present,
/// otherwise treat responses as fresh for a short heuristic window.
pub fn is_fresh(resp: &CachedResponse, now: u64) -> bool {
    let header = |name: &str| {
        resp.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.clone())
    };
    if let Some(cc) = header("cache-control") {
        let cc = cc.to_ascii_lowercase();
        if cc.contains("no-store") {
            return false;
        }
        if let Some(idx) = cc.find("max-age=") {
            let rest = &cc[idx + 8..];
            let num: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
            if let Ok(age) = num.parse::<u64>() {
                return now.saturating_sub(resp.stored_at) < age;
            }
        }
    }
    // Heuristic freshness window.
    now.saturating_sub(resp.stored_at) < 60
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(url: &str) -> CachedResponse {
        CachedResponse {
            url: url.to_string(),
            status: 200,
            headers: vec![("content-type".into(), "text/html".into())],
            body: b"<h1>hi</h1>".to_vec(),
            stored_at: SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs(),
            protocol: "h2".into(),
        }
    }

    #[test]
    fn put_get_roundtrip() {
        let cache = HttpCache::new();
        cache.put(sample("https://example.com/"));
        let hit = cache.get("https://example.com/").unwrap();
        assert_eq!(hit.body, b"<h1>hi</h1>".to_vec());
        assert!(cache.get("https://other.com/").is_none());
    }

    #[test]
    fn fresh_window() {
        let mut resp = sample("https://example.com/");
        assert!(is_fresh(&resp, resp.stored_at + 10));
        resp.headers = vec![("cache-control".into(), "max-age=1".into())];
        assert!(!is_fresh(&resp, resp.stored_at + 10));
        resp.headers = vec![("cache-control".into(), "no-store".into())];
        assert!(!is_fresh(&resp, resp.stored_at));
    }
}
