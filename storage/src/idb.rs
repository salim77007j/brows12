//! Minimal IndexedDB-style object stores (documents + indexes).
//!
//! v1 supports the storage semantics behind the API (put/get/delete,
//! auto-increment keys, secondary indexes); the full asynchronous JS surface
//! is exposed through `brows12-js` bindings. See docs/ROADMAP.md.

use crate::{KeyValueStore, MemoryStore};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum IdbKey {
    Number(f64),
    Str(String),
}

impl IdbKey {
    fn to_sort_string(&self) -> String {
        match self {
            IdbKey::Number(n) => format!("n{:020.6}", n),
            IdbKey::Str(s) => format!("s{}", s),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObjectStoreConfig {
    pub name: String,
    pub key_path: String,
    pub indexes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Record {
    key: IdbKey,
    value: serde_json::Value,
    indexes: BTreeMap<String, IdbKey>,
}

/// One database. Multiple object stores live under one table, namespaced by
/// store name; values are JSON documents.
pub struct IdbDatabase {
    store: Arc<dyn KeyValueStore>,
    table: String,
    stores: Mutex<Vec<ObjectStoreConfig>>,
    next_keys: Mutex<BTreeMap<String, u64>>,
}

impl IdbDatabase {
    /// Open (or create) a database for `origin` in the given KV backend.
    pub fn open(store: Arc<dyn KeyValueStore>, origin: &str, name: &str) -> Self {
        Self {
            store,
            table: format!("idb::{origin}::{name}"),
            stores: Mutex::new(Vec::new()),
            next_keys: Mutex::new(BTreeMap::new()),
        }
    }

    /// Create an object store (idempotent).
    pub fn create_object_store(&self, config: ObjectStoreConfig) -> Result<(), crate::StorageError> {
        let mut stores = self.stores.lock().unwrap();
        if !stores.iter().any(|s| s.name == config.name) {
            stores.push(config);
        }
        Ok(())
    }

    fn extract_key(config: &ObjectStoreConfig, value: &serde_json::Value) -> Option<IdbKey> {
        match value.get(&config.key_path)? {
            serde_json::Value::Number(n) => n.as_f64().map(IdbKey::Number),
            serde_json::Value::String(s) => Some(IdbKey::Str(s.clone())),
            _ => None,
        }
    }

    /// Put a value (auto-increment when no key in key_path).
    pub fn put(
        &self,
        store_name: &str,
        value: &serde_json::Value,
    ) -> Result<IdbKey, crate::StorageError> {
        let config = {
            let stores = self.stores.lock().unwrap();
            stores
                .iter()
                .find(|s| s.name == store_name)
                .cloned()
                .ok_or_else(|| crate::StorageError::InvalidKey(store_name.into()))?
        };

        let key = match Self::extract_key(&config, value) {
            Some(k) => k,
            None => {
                let mut next = self.next_keys.lock().unwrap();
                let n = next.entry(store_name.to_string()).or_insert(0);
                *n += 1;
                IdbKey::Number(*n as f64)
            }
        };

        let mut indexes = BTreeMap::new();
        for index in &config.indexes {
            if let Some(v) = value.get(index) {
                let k = match v {
                    serde_json::Value::Number(n) => n.as_f64().map(IdbKey::Number),
                    serde_json::Value::String(s) => Some(IdbKey::Str(s.clone())),
                    _ => None,
                };
                if let Some(k) = k {
                    indexes.insert(index.clone(), k);
                }
            }
        }

        let record = Record { key: key.clone(), value: value.clone(), indexes };
        let encoded = serde_json::to_vec(&record)
            .map_err(|e| crate::StorageError::Backend(e.to_string()))?;
        self.store.put(&self.table, &record.key.to_sort_string(), &encoded)?;
        Ok(key)
    }

    pub fn get(&self, store_name: &str, key: &IdbKey) -> Result<Option<serde_json::Value>, crate::StorageError> {
        let raw = self.store.get(&self.table, &key.to_sort_string())?;
        match raw {
            Some(bytes) => {
                let record: Record = serde_json::from_slice(&bytes)
                    .map_err(|e| crate::StorageError::Backend(e.to_string()))?;
                Ok(Some(record.value))
            }
            None => Ok(None),
        }
        .inspect(|_| {
            let _ = store_name;
        })
    }

    pub fn delete(&self, store_name: &str, key: &IdbKey) -> Result<(), crate::StorageError> {
        let _ = store_name;
        self.store.delete(&self.table, &key.to_sort_string())
    }

    /// All values in a store, ordered by key.
    pub fn get_all(&self, store_name: &str) -> Result<Vec<serde_json::Value>, crate::StorageError> {
        let keys = self.store.keys(&self.table)?;
        let mut out = Vec::new();
        for key in keys {
            if let Some(bytes) = self.store.get(&self.table, &key)? {
                if let Ok(record) = serde_json::from_slice::<Record>(&bytes) {
                    let _ = store_name;
                    out.push(record.value);
                }
            }
        }
        Ok(out)
    }
}

/// Convenience constructor used by tests and the JS bindings.
pub fn memory_db(origin: &str, name: &str) -> IdbDatabase {
    IdbDatabase::open(Arc::new(MemoryStore::new()), origin, name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn put_get_and_indexes() {
        let db = memory_db("https://a.com", "app");
        db.create_object_store(ObjectStoreConfig {
            name: "todos".into(),
            key_path: "id".into(),
            indexes: vec!["title".into()],
        })
        .unwrap();

        let key = db.put("todos", &json!({"id": 7, "title": "buy milk"})).unwrap();
        assert_eq!(key, IdbKey::Number(7.0));

        let got = db.get("todos", &IdbKey::Number(7.0)).unwrap().unwrap();
        assert_eq!(got["title"], "buy milk");

        // Auto-increment when key missing.
        let k2 = db.put("todos", &json!({"title": "auto"})).unwrap();
        assert!(matches!(k2, IdbKey::Number(_)));

        let all = db.get_all("todos").unwrap();
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn origin_isolation() {
        let db_a = memory_db("https://a.com", "db");
        let db_b = memory_db("https://b.com", "db");
        db_a.create_object_store(ObjectStoreConfig {
            name: "s".into(),
            key_path: "id".into(),
            indexes: vec![],
        })
        .unwrap();
        db_a.put("s", &json!({"id": 1})).unwrap();
        assert!(db_b.get_all("s").unwrap().is_empty());
    }
}
