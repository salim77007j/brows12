//! # brows12-storage
//!
//! Persistence layer for the Brows12 browser engine.
//!
//! * [`cookies`] — RFC 6265bis cookie jar with CHIPS-style partitioning.
//! * [`kv`] — key/value store: in-memory by default, `redb` on disk.
//! * [`webstorage`] — `localStorage` / `sessionStorage` per origin.
//! * [`cache`] — HTTP response cache (memory + optional disk, LRU).
//! * [`idb`] — minimal IndexedDB object stores.

pub mod cache;
pub mod cookies;
pub mod idb;
pub mod kv;
pub mod webstorage;

pub use cookies::{Cookie, CookieJar, SameSite};
pub use idb::{IdbDatabase, IdbKey};
pub use kv::{KeyValueStore, MemoryStore};
pub use webstorage::WebStorage;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("storage backend error: {0}")]
    Backend(String),
    #[error("invalid key: {0}")]
    InvalidKey(String),
    #[error("quota exceeded")]
    QuotaExceeded,
}

/// eTLD+1 computation via the Public Suffix List (`psl` crate). Used for
/// cookie domain checks and CHIPS partition keys.
pub fn registrable_domain(host: &str) -> String {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    if host.is_empty() {
        return String::new();
    }
    // IP addresses and localhost have no registrable domain.
    if host.parse::<std::net::IpAddr>().is_ok() || host == "localhost" {
        return host;
    }
    let bytes = host.as_bytes();
    match psl::domain(bytes) {
        Some(d) => String::from_utf8_lossy(d.as_bytes()).to_string(),
        None => host, // PSL entry itself (e.g. "com") — fall back
    }
}
