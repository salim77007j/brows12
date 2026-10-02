//! WebGPU natives: `__brows12.gpu*` functions called from glue.js.
//!
//! The device is the shared engine wgpu device (see webgl.rs gpu_shared);
//! `requestAdapter` returns null on GPU-less environments like getContext.

use crate::environment::JsEnvironment;
use crate::webgpu::{usage, GpuDevice};
use rquickjs::{Ctx, Function, Object, Value};
use std::sync::{Arc, Mutex, OnceLock};

static DEVICE: OnceLock<Option<Arc<GpuDevice>>> = OnceLock::new();

fn device() -> Option<Arc<GpuDevice>> {
    DEVICE.get_or_init(GpuDevice::request).clone()
}

macro_rules! dev {
    () => {
        match device() {
            Some(d) => d,
            None => return Default::default(),
        }
    };
}

/// Install the `__brows12` WebGPU natives.
pub fn install<'js>(ctx: &Ctx<'js>, _env: &Arc<JsEnvironment>, ns: &Object<'js>) -> rquickjs::Result<()> {
    let _ = ctx;
    let _ = _env;

    // adapter/device ---------------------------------------------------------
    ns.set(
        "gpuRequestAdapter",
        Function::new(ctx.clone(), move || -> Option<String> {
            let d = device()?;
            Some(d.adapter_name.clone())
        }),
    )?;
    ns.set(
        "gpuCreateBuffer",
        Function::new(ctx.clone(), move |size: f64, usage_bits: u32| -> Option<u64> {
            let d = dev!();
            let b = d.create_buffer(size.max(4.0) as u64, usage_bits);
            Some(b.id)
        }),
    )?;
    ns.set(
        "gpuWriteBuffer",
        Function::new(ctx.clone(), move |buf: u64, offset: f64, data: Value| {
            let d = dev!();
            if let Some(bytes) = crate::glue::as_typed_u8(&data) {
                d.write_buffer(buf, offset.max(0.0) as u64, &bytes);
            } else if let Some(vals) = crate::glue::as_typed_f32(&data) {
                let mut bytes = Vec::with_capacity(vals.len() * 4);
                for v in vals {
                    bytes.extend_from_slice(&v.to_le_bytes());
                }
                d.write_buffer(buf, offset.max(0.0) as u64, &bytes);
            }
        }),
    )?;
    ns.set(
        "gpuMapReadBuffer",
        Function::new(ctx.clone(), move |buf: u64| -> Option<Vec<u8>> {
            let d = dev!();
            d.map_read(buf)
        }),
    )?;
    ns.set(
        "gpuDestroyBuffer",
        Function::new(ctx.clone(), move |buf: u64| {
            let d = dev!();
            d.destroy_buffer(buf);
        }),
    )?;
    ns.set(
        "gpuCreateComputePipeline",
        Function::new(ctx.clone(), move |code: String, entry: String| -> Option<u64> {
            let d = device()?;
            match d.create_compute_pipeline(&code, &entry) {
                Ok(id) => Some(id),
                Err(msg) => {
                    d.errors.push(msg);
                    None
                }
            }
        }),
    )?;
    ns.set(
        "gpuCreateBindGroupLayout",
        Function::new(
            ctx.clone(),
            move |entries: Vec<Value>| -> Option<u64> {
                let d = device()?;
                let mut parsed: Vec<(u32, u32, String)> = Vec::new();
                for e in &entries {
                    let obj = e.as_object()?;
                    let binding: u32 = obj.get("binding").ok()?;
                    let visibility: u32 = obj.get("visibility").ok()?;
                    let ty: String = obj
                        .get::<_, String>("bufferType")
                        .unwrap_or_else(|_| "uniform".to_string());
                    parsed.push((binding, visibility, ty));
                }
                let refs: Vec<(u32, u32, &str)> =
                    parsed.iter().map(|(b, v, t)| (*b, *v, t.as_str())).collect();
                match d.create_bind_group_layout(&refs) {
                    Ok(id) => Some(id),
                    Err(msg) => {
                        d.errors.push(msg);
                        None
                    }
                }
            },
        ),
    )?;
    ns.set(
        "gpuCreateBindGroup",
        Function::new(
            ctx.clone(),
            move |layout: u64, entries: Vec<Value>| -> Option<u64> {
                let d = device()?;
                let mut parsed: Vec<(u32, u64, u64, Option<u64>)> = Vec::new();
                for e in &entries {
                    let obj = e.as_object()?;
                    let binding: u32 = obj.get("binding").ok()?;
                    let buffer: u64 = obj.get("buffer").ok()?;
                    let offset: u64 = obj.get::<_, f64>("offset").unwrap_or(0.0).max(0.0) as u64;
                    let size: Option<u64> = obj
                        .get::<_, Option<f64>>("size")
                        .unwrap_or(None)
                        .filter(|s| *s > 0.0)
                        .map(|s| s as u64);
                    parsed.push((binding, buffer, offset, size));
                }
                match d.create_bind_group(layout, &parsed) {
                    Ok(id) => Some(id),
                    Err(msg) => {
                        d.errors.push(msg);
                        None
                    }
                }
            },
        ),
    )?;
    ns.set(
        "gpuSubmitCompute",
        Function::new(ctx.clone(), move |steps: Vec<Value>| {
            let d = dev!();
            let mut plan: Vec<(String, u64, u32, u32, u32)> = Vec::new();
            for s in &steps {
                if let Some(obj) = s.as_object() {
                    let op: String = obj.get("op").unwrap_or_default();
                    let id: u64 = obj.get::<_, f64>("id").unwrap_or(0.0) as u64;
                    let x: u32 = obj.get::<_, f64>("x").unwrap_or(1.0) as u32;
                    let y: u32 = obj.get::<_, f64>("y").unwrap_or(1.0) as u32;
                    let z: u32 = obj.get::<_, f64>("z").unwrap_or(1.0) as u32;
                    plan.push((op, id, x, y, z));
                }
            }
            d.submit_compute_steps(&plan);
        }),
    )?;
    ns.set(
        "gpuGetErrors",
        Function::new(ctx.clone(), move || -> Vec<String> {
            match device() {
                Some(d) => d.errors.drain(),
                None => Vec::new(),
            }
        }),
    )?;
    ns.set(
        "gpuUsage",
        Function::new(ctx.clone(), move |name: String| -> u32 {
            match name.as_str() {
                "MAP_READ" => usage::MAP_READ,
                "MAP_WRITE" => usage::MAP_WRITE,
                "COPY_SRC" => usage::COPY_SRC,
                "COPY_DST" => usage::COPY_DST,
                "INDEX" => usage::INDEX,
                "VERTEX" => usage::VERTEX,
                "UNIFORM" => usage::UNIFORM,
                "STORAGE" => usage::STORAGE,
                "INDIRECT" => usage::INDIRECT,
                _ => 0,
            }
        }),
    )?;
    Ok(())
}
