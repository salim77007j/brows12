//! IndexedDB natives: `__brows12.idb*` functions called from glue.js.
//!
//! Backed by `brows12_storage::IdbDatabase` over the engine's KV store.
//! Documented deviation for v0.2: the surface is synchronous (like the rest
//! of the engine's storage seam) — `open`/`transaction`/requests resolve
//! immediately instead of returning IDBRequest/Promise objects. Schema
//! upgrades run through `onupgradeneeded`-equivalent callbacks on the JS
//! side during the synchronous open.

use crate::environment::JsEnvironment;
use brows12_storage::idb::ObjectStoreConfig;
use brows12_storage::IdbDatabase;
use rquickjs::{Ctx, Function, Object};
use std::sync::Arc;

fn key_to_json(key: f64, is_string: bool, raw: &str) -> serde_json::Value {
    if is_string {
        serde_json::Value::String(raw.to_string())
    } else {
        serde_json::Value::Number(
            serde_json::Number::from_f64(key).unwrap_or(serde_json::Number::from(0)),
        )
    }
}

/// Install the `__brows12` IndexedDB natives.
pub fn install<'js>(ctx: &Ctx<'js>, env: &Arc<JsEnvironment>, ns: &Object<'js>) -> rquickjs::Result<()> {
    let kv = env.kv.clone();

    ns.set(
        "idbOpen",
        Function::new(ctx.clone(), {
            let env = env.clone();
            move |name: String| -> String {
                let mut reg = env.idb_registry.lock().unwrap();
                if !reg.contains_key(&name) {
                    let db = Arc::new(IdbDatabase::open(kv.clone(), &env.base_url, &name));
                    reg.insert(name.clone(), db);
                }
                name
            }
        }),
    )?;

    ns.set(
        "idbCreateObjectStore",
        Function::new(ctx.clone(), {
            let env = env.clone();
            move |db: String, store: String, key_path: String| -> bool {
                let reg = env.idb_registry.lock().unwrap();
                match reg.get(&db) {
                    Some(database) => database
                        .create_object_store(ObjectStoreConfig {
                            name: store,
                            key_path,
                            indexes: Vec::new(),
                        })
                        .is_ok(),
                    None => false,
                }
            }
        }),
    )?;

    ns.set(
        "idbPut",
        Function::new(ctx.clone(), {
            let env = env.clone();
            move |db: String, store: String, key_path: String, key: f64, is_string: bool, raw_key: String, value_json: String| -> bool {
                let reg = env.idb_registry.lock().unwrap();
                let Some(database) = reg.get(&db) else { return false };
                let Ok(mut value) = serde_json::from_str::<serde_json::Value>(&value_json) else {
                    return false;
                };
                // Explicit keys are folded into the key_path field so the
                // storage layer can extract them.
                if let Some(field) = value.as_object_mut() {
                    field.entry(key_path.clone()).or_insert(key_to_json(key, is_string, &raw_key));
                }
                database.put(&store, &value).is_ok()
            }
        }),
    )?;

    ns.set(
        "idbGet",
        Function::new(ctx.clone(), {
            let env = env.clone();
            move |db: String, store: String, key: f64, is_string: bool, raw_key: String| -> Option<String> {
                let reg = env.idb_registry.lock().unwrap();
                let database = reg.get(&db)?;
                let idb_key = if is_string {
                    brows12_storage::idb::IdbKey::Str(raw_key)
                } else {
                    brows12_storage::idb::IdbKey::Number(key)
                };
                database
                    .get(&store, &idb_key)
                    .ok()
                    .flatten()
                    .map(|v| v.to_string())
            }
        }),
    )?;

    ns.set(
        "idbGetAll",
        Function::new(ctx.clone(), {
            let env = env.clone();
            move |db: String, store: String| -> Vec<String> {
                let reg = env.idb_registry.lock().unwrap();
                match reg.get(&db) {
                    Some(database) => database
                        .get_all(&store)
                        .unwrap_or_default()
                        .into_iter()
                        .map(|v| v.to_string())
                        .collect(),
                    None => Vec::new(),
                }
            }
        }),
    )?;

    ns.set(
        "idbDelete",
        Function::new(ctx.clone(), {
            let env = env.clone();
            move |db: String, store: String, key: f64, is_string: bool, raw_key: String| -> bool {
                let reg = env.idb_registry.lock().unwrap();
                let Some(database) = reg.get(&db) else { return false };
                let idb_key = if is_string {
                    brows12_storage::idb::IdbKey::Str(raw_key)
                } else {
                    brows12_storage::idb::IdbKey::Number(key)
                };
                database.delete(&store, &idb_key).is_ok()
            }
        }),
    )?;
    Ok(())
}
