//! `localStorage` / `sessionStorage` per origin.

use crate::{KeyValueStore, MemoryStore, StorageError};
use std::sync::Arc;

const QUOTA_BYTES: usize = 5 * 1024 * 1024;

/// A web storage area bound to an origin, backed by a [`KeyValueStore`].
/// Table layout: `<kind>::<origin>` so all origins live in one file.
#[derive(Clone)]
pub struct WebStorage {
    store: Arc<dyn KeyValueStore>,
    kind: &'static str,
    origin: String,
}

impl std::fmt::Debug for WebStorage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebStorage")
            .field("kind", &self.kind)
            .field("origin", &self.origin)
            .finish()
    }
}

impl WebStorage {
    /// Persistent localStorage area for `origin`.
    pub fn local(store: Arc<dyn KeyValueStore>, origin: &str) -> Self {
        Self { store, kind: "local", origin: origin.to_string() }
    }

    /// Session storage area (same API; engines may choose a memory store).
    pub fn session(origin: &str) -> Self {
        Self { store: Arc::new(MemoryStore::new()), kind: "session", origin: origin.to_string() }
    }

    fn table(&self) -> String {
        format!("{}::{}", self.kind, self.origin)
    }

    pub fn get_item(&self, key: &str) -> Result<Option<String>, StorageError> {
        let raw = self.store.get(&self.table(), key)?;
        Ok(raw.and_then(|b| String::from_utf8(b).ok()))
    }

    pub fn set_item(&self, key: &str, value: &str) -> Result<(), StorageError> {
        let bytes_len = key.len() + value.len();
        if bytes_len > QUOTA_BYTES {
            return Err(StorageError::QuotaExceeded);
        }
        self.store.put(&self.table(), key, value.as_bytes())
    }

    pub fn remove_item(&self, key: &str) -> Result<(), StorageError> {
        self.store.delete(&self.table(), key)
    }

    pub fn clear(&self) -> Result<(), StorageError> {
        self.store.clear(&self.table())
    }

    pub fn length(&self) -> Result<usize, StorageError> {
        Ok(self.keys()?.len())
    }

    pub fn keys(&self) -> Result<Vec<String>, StorageError> {
        self.store.keys(&self.table())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origins_are_isolated() {
        let store = Arc::new(MemoryStore::new());
        let a = WebStorage::local(store.clone(), "https://a.com");
        let b = WebStorage::local(store, "https://b.com");
        a.set_item("k", "a-value").unwrap();
        assert_eq!(b.get_item("k").unwrap(), None);
        assert_eq!(a.get_item("k").unwrap().as_deref(), Some("a-value"));
        b.set_item("k", "b-value").unwrap();
        assert_eq!(a.get_item("k").unwrap().as_deref(), Some("a-value"));
    }

    #[test]
    fn clear_and_length() {
        let a = WebStorage::session("https://a.com");
        a.set_item("1", "x").unwrap();
        a.set_item("2", "y").unwrap();
        assert_eq!(a.length().unwrap(), 2);
        a.clear().unwrap();
        assert_eq!(a.length().unwrap(), 0);
    }
}
