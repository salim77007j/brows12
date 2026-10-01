//! Key/value store abstraction: memory (default) or redb (disk).

use crate::StorageError;
use std::collections::HashMap;
use std::sync::RwLock;

/// Backend-agnostic KV API used by web storage, the HTTP cache and IndexedDB.
pub trait KeyValueStore: Send + Sync {
    fn get(&self, table: &str, key: &str) -> Result<Option<Vec<u8>>, StorageError>;
    fn put(&self, table: &str, key: &str, value: &[u8]) -> Result<(), StorageError>;
    fn delete(&self, table: &str, key: &str) -> Result<(), StorageError>;
    /// All keys in a table (sorted, for deterministic iteration).
    fn keys(&self, table: &str) -> Result<Vec<String>, StorageError>;
    fn clear(&self, table: &str) -> Result<(), StorageError>;
}

/// Pure in-memory store. Zero syscalls, zero fsyncs — the default backend.
#[derive(Debug, Default)]
pub struct MemoryStore {
    tables: RwLock<HashMap<String, HashMap<String, Vec<u8>>>>,
}

impl MemoryStore {
    pub fn new() -> Self {
        Self::default()
    }
}

impl KeyValueStore for MemoryStore {
    fn get(&self, table: &str, key: &str) -> Result<Option<Vec<u8>>, StorageError> {
        let guard = self
            .tables
            .read()
            .map_err(|_| StorageError::Backend("lock poisoned".into()))?;
        Ok(guard.get(table).and_then(|t| t.get(key).cloned()))
    }

    fn put(&self, table: &str, key: &str, value: &[u8]) -> Result<(), StorageError> {
        let mut guard = self
            .tables
            .write()
            .map_err(|_| StorageError::Backend("lock poisoned".into()))?;
        guard.entry(table.to_string()).or_default().insert(key.to_string(), value.to_vec());
        Ok(())
    }

    fn delete(&self, table: &str, key: &str) -> Result<(), StorageError> {
        let mut guard = self
            .tables
            .write()
            .map_err(|_| StorageError::Backend("lock poisoned".into()))?;
        if let Some(t) = guard.get_mut(table) {
            t.remove(key);
        }
        Ok(())
    }

    fn keys(&self, table: &str) -> Result<Vec<String>, StorageError> {
        let guard = self
            .tables
            .read()
            .map_err(|_| StorageError::Backend("lock poisoned".into()))?;
        let mut keys: Vec<String> = guard
            .get(table)
            .map(|t| t.keys().cloned().collect())
            .unwrap_or_default();
        keys.sort();
        Ok(keys)
    }

    fn clear(&self, table: &str) -> Result<(), StorageError> {
        let mut guard = self
            .tables
            .write()
            .map_err(|_| StorageError::Backend("lock poisoned".into()))?;
        guard.remove(table);
        Ok(())
    }
}

/// Durable store backed by redb (single file, ACID, pure Rust).
#[cfg(feature = "persist")]
pub mod redb_store {
    use super::{KeyValueStore, StorageError};
    use redb::{Database, ReadableDatabase, ReadableTable, TableDefinition};
    use std::path::Path;
    use std::sync::RwLock;

    const TABLE: TableDefinition<(&str, &str), &[u8]> = TableDefinition::new("brows12_kv");

    /// redb-backed store. Cheap to clone (connection shared internally).
    pub struct RedbStore {
        db: Database,
        write_lock: RwLock<()>,
    }

    impl RedbStore {
        pub fn open(path: impl AsRef<Path>) -> Result<Self, StorageError> {
            let db = Database::create(path)
                .map_err(|e| StorageError::Backend(format!("redb open: {e}")))?;
            Ok(Self { db, write_lock: RwLock::new(()) })
        }

        fn table_name(table: &str) -> String {
            format!("brows12::{table}")
        }
    }

    impl KeyValueStore for RedbStore {
        fn get(&self, table: &str, key: &str) -> Result<Option<Vec<u8>>, StorageError> {
            let read = self
                .db
                .begin_read()
                .map_err(|e| StorageError::Backend(e.to_string()))?;
            let t = read
                .open_table(TABLE)
                .map_err(|e| StorageError::Backend(e.to_string()))?;
            let full = Self::table_name(table);
            Ok(t.get((full.as_str(), key)).ok().flatten().map(|v| v.value().to_vec()))
        }

        fn put(&self, table: &str, key: &str, value: &[u8]) -> Result<(), StorageError> {
            let _g = self.write_lock.write().unwrap();
            let write = self
                .db
                .begin_write()
                .map_err(|e| StorageError::Backend(e.to_string()))?;
            {
                let mut t = write
                    .open_table(TABLE)
                    .map_err(|e| StorageError::Backend(e.to_string()))?;
                let full = Self::table_name(table);
                t.insert((full.as_str(), key), value)
                    .map_err(|e| StorageError::Backend(e.to_string()))?;
            }
            write
                .commit()
                .map_err(|e| StorageError::Backend(e.to_string()))
        }

        fn delete(&self, table: &str, key: &str) -> Result<(), StorageError> {
            let _g = self.write_lock.write().unwrap();
            let write = self
                .db
                .begin_write()
                .map_err(|e| StorageError::Backend(e.to_string()))?;
            {
                let mut t = write
                    .open_table(TABLE)
                    .map_err(|e| StorageError::Backend(e.to_string()))?;
                let full = Self::table_name(table);
                t.remove((full.as_str(), key))
                    .map_err(|e| StorageError::Backend(e.to_string()))?;
            }
            write
                .commit()
                .map_err(|e| StorageError::Backend(e.to_string()))
        }

        fn keys(&self, table: &str) -> Result<Vec<String>, StorageError> {
            let read = self
                .db
                .begin_read()
                .map_err(|e| StorageError::Backend(e.to_string()))?;
            let t = read
                .open_table(TABLE)
                .map_err(|e| StorageError::Backend(e.to_string()))?;
            let full = Self::table_name(table);
            let mut out = Vec::new();
            for row in t.iter().map_err(|e| StorageError::Backend(e.to_string()))? {
                let (k, _) = row.map_err(|e| StorageError::Backend(e.to_string()))?;
                let (tbl, key) = k.value();
                if tbl == full {
                    out.push(key.to_string());
                }
            }
            out.sort();
            Ok(out)
        }

        fn clear(&self, table: &str) -> Result<(), StorageError> {
            let keys = self.keys(table)?;
            for k in keys {
                self.delete(table, &k)?;
            }
            Ok(())
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn roundtrip() {
            let dir = tempfile::tempdir().unwrap();
            let store = RedbStore::open(dir.path().join("db.redb")).unwrap();
            store.put("t", "a", b"1").unwrap();
            store.put("t", "b", b"2").unwrap();
            assert_eq!(store.get("t", "a").unwrap().unwrap(), b"1");
            assert_eq!(store.keys("t").unwrap(), vec!["a".to_string(), "b".to_string()]);
            store.delete("t", "a").unwrap();
            assert_eq!(store.get("t", "a").unwrap(), None);
        }
    }
}
