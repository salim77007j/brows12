//! WebAssembly support via `wasmi` (in-process interpreter).
//!
//! Choice rationale (documented in LIBRARY_CHOICES.md): wasmi is a mature,
//! deterministic, no-JIT interpreter — it builds quickly on constrained CI,
//! avoids RWX pages in hardened containers, and its end-to-end latency for
//! small modules beats JIT compilation on the 2-core targets this engine
//! ships for. wasmtime remains an optional swap for peak JIT throughput.
//!
//! Transfer model: module bytes arrive base64-encoded, functions are called
//! by name with JSON argument lists, memory is read/written through base64
//! slices. This preserves the "plain data across the boundary" invariant of
//! the realm design (no JS values stored on the Rust side).

use crate::environment::JsEnvironment;
use rquickjs::{Ctx, Function, Object};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

struct WasmHandle {
    store: wasmi::Store<()>,
    instance: wasmi::Instance,
    funcs: Vec<String>,
    memory: Option<wasmi::Memory>,
}

#[derive(Default)]
pub struct WasmStore {
    instances: Mutex<HashMap<i32, WasmHandle>>,
}

impl WasmStore {
    pub fn new() -> Self {
        Self::default()
    }
}

fn val_from_f64(v: f64, ty: &wasmi::ValType) -> wasmi::Val {
    match ty {
        wasmi::ValType::I32 => wasmi::Val::I32(v as i32),
        wasmi::ValType::I64 => wasmi::Val::I64(v as i64),
        wasmi::ValType::F32 => wasmi::Val::F32((v as f32).into()),
        wasmi::ValType::F64 => wasmi::Val::F64(v.into()),
        other => {
            tracing::debug!(?other, "wasm param type unsupported");
            wasmi::Val::I32(0)
        }
    }
}

fn val_to_f64(v: &wasmi::Val) -> Option<f64> {
    match v {
        wasmi::Val::I32(x) => Some(*x as f64),
        wasmi::Val::I64(x) => Some(*x as f64),
        wasmi::Val::F32(x) => Some(f64::from(f32::from(*x))),
        wasmi::Val::F64(x) => Some(f64::from(f64::from(*x))),
        _ => None,
    }
}

/// Install `__brows12` wasm natives.
pub fn install<'js>(
    ctx: &Ctx<'js>,
    env: &Arc<JsEnvironment>,
    ns: &Object<'js>,
) -> rquickjs::Result<()> {
    let store = Arc::new(WasmStore::new());
    let _ = env;

    ns.set(
        "wasmNew",
        Function::new(ctx.clone(), {
            let store = store.clone();
            move |id: i32, bytes_b64: String| -> String {
                let bytes = match crate::glue_b64::decode(&bytes_b64) {
                    Some(b) => b,
                    None => return r#"{"ok":false,"error":"bad base64"}"#.into(),
                };
                let engine = wasmi::Engine::default();
                let module = match wasmi::Module::new(&engine, &bytes[..]) {
                    Ok(m) => m,
                    Err(e) => {
                        return format!(
                            r#"{{"ok":false,"error":"{}"}}"#,
                            e.to_string().replace('"', "'")
                        )
                    }
                };
                let mut wasm_store = wasmi::Store::new(&engine, ());
                // v0.2: pure computation modules (no host imports).
                let linker: wasmi::Linker<()> = wasmi::Linker::new(&engine);
                let instance = match linker.instantiate_and_start(&mut wasm_store, &module) {
                    Ok(i) => i,
                    Err(e) => {
                        return format!(
                            r#"{{"ok":false,"error":"{}"}}"#,
                            e.to_string().replace('"', "'")
                        )
                    }
                };

                let mut funcs = Vec::new();
                let mut memory = None;
                for export in instance.exports(&wasm_store) {
                    let name = export.name().to_string();
                    match export.into_extern() {
                        wasmi::Extern::Func(f) => funcs.push(name),
                        wasmi::Extern::Memory(m) => memory = Some(m),
                        _ => {}
                    }
                }
                let has_memory = memory.is_some();
                store.instances.lock().unwrap().insert(
                    id,
                    WasmHandle { store: wasm_store, instance, funcs: funcs.clone(), memory },
                );
                let funcs_json = serde_json::to_string(&funcs).unwrap_or_else(|_| "[]".into());
                format!(r#"{{"ok":true,"id":{id},"hasMemory":{has_memory},"funcs":{funcs_json}}}"#)
            }
        })?,
    )?;

    ns.set(
        "wasmCall",
        Function::new(ctx.clone(), {
            let store = store.clone();
            move |id: i32, name: String, args_json: String| -> String {
                let mut map = store.instances.lock().unwrap();
                let Some(handle) = map.get_mut(&id) else {
                    return r#"{"ok":false,"error":"no instance"}"#.into();
                };
                let Some(func) = handle
                    .instance
                    .get_export(&handle.store, &name)
                    .and_then(|e| e.into_func())
                else {
                    return format!(r#"{{"ok":false,"error":"no export {name}"}}"#);
                };
                let ty = func.ty(&handle.store);
                let args: Vec<f64> =
                    serde_json::from_str::<Vec<f64>>(&args_json).unwrap_or_default();
                if ty.params().len() != args.len() {
                    return format!(
                        r#"{{"ok":false,"error":"arity {}!={}"}}"#,
                        args.len(),
                        ty.params().len()
                    );
                }
                let inputs: Vec<wasmi::Val> = args
                    .iter()
                    .zip(ty.params().iter())
                    .map(|(v, t)| val_from_f64(*v, t))
                    .collect();
                let mut outputs = vec![wasmi::Val::I32(0); ty.results().len()];
                match func.call(&mut handle.store, &inputs, &mut outputs) {
                    Ok(()) => {
                        let results: Vec<f64> =
                            outputs.iter().filter_map(val_to_f64).collect();
                        let results_json =
                            serde_json::to_string(&results).unwrap_or_else(|_| "[]".into());
                        format!(r#"{{"ok":true,"results":{results_json}}}"#)
                    }
                    Err(e) => format!(
                        r#"{{"ok":false,"error":"{}"}}"#,
                        e.to_string().replace('"', "'")
                    ),
                }
            }
        })?,
    )?;

    ns.set(
        "wasmMemoryRead",
        Function::new(ctx.clone(), {
            let store = store.clone();
            move |id: i32, offset: i64, len: i64| -> String {
                let mut map = store.instances.lock().unwrap();
                let Some(handle) = map.get_mut(&id) else {
                    return String::new();
                };
                let Some(mem) = handle.memory else { return String::new() };
                let data = mem.data(&handle.store);
                let start = (offset.max(0) as usize).min(data.len());
                let end = ((offset + len).max(0) as usize).min(data.len());
                crate::glue_b64::encode(&data[start..end])

            }
        })?,
    )?;

    ns.set(
        "wasmMemoryWrite",
        Function::new(ctx.clone(), {
            let store = store.clone();
            move |id: i32, offset: i64, bytes_b64: String| -> bool {
                let mut map = store.instances.lock().unwrap();
                let Some(handle) = map.get_mut(&id) else { return false };
                let Some(mem) = handle.memory else { return false };
                let Some(bytes) = crate::glue_b64::decode(&bytes_b64) else { return false };
                use wasmi::AsContextMut as _;
                let start = offset.max(0) as usize;
                let data = mem.data_mut(&mut handle.store);
                if start + bytes.len() > data.len() {
                    return false;
                }
                data[start..start + bytes.len()].copy_from_slice(&bytes);
                true
            }
        })?,
    )?;

    ns.set(
        "wasmDrop",
        Function::new(ctx.clone(), {
            let store = store.clone();
            move |id: i32| {
                store.instances.lock().unwrap().remove(&id);
            }
        })?,
    )?;

    Ok(())
}
