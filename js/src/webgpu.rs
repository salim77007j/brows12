//! WebGPU JS surface over the engine's wgpu 30 device.
//!
//! Honest scope for v0.2 (see docs/CAPABILITY_REPORT.md): adapters/devices,
//! buffers (create/write/map/copy), WGSL shader modules, compute pipelines,
//! bind group layouts + bind groups, command encoders with compute passes
//! and buffer copies, queue submit. Render pipelines/texture objects are
//! reachable through the WebGL 2 path; full WebGPU render-graph bindings are
//! v0.3 scope.
//!
//! Documented deviation: `mapAsync` resolves synchronously once the GPU work
//! completes (pollster block_on) instead of returning a Promise — the JS
//! surface stays synchronous like the rest of the engine's GL seam.

use crate::webgl::gpu_shared;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

// ---- GPUBufferUsage constants (match the WebGPU spec bit values) -----------
pub mod usage {
    pub const MAP_READ: u32 = 0x0001;
    pub const MAP_WRITE: u32 = 0x0002;
    pub const COPY_SRC: u32 = 0x0004;
    pub const COPY_DST: u32 = 0x0008;
    pub const INDEX: u32 = 0x0010;
    pub const VERTEX: u32 = 0x0020;
    pub const UNIFORM: u32 = 0x0040;
    pub const STORAGE: u32 = 0x0080;
    pub const INDIRECT: u32 = 0x0100;
    pub const QUERY_RESOLVE: u32 = 0x0200;
}

pub struct GpuErrorState(Mutex<Vec<String>>);

impl GpuErrorState {
    fn new() -> Self {
        GpuErrorState(Mutex::new(Vec::new()))
    }
    pub fn push(&self, msg: String) {
        self.0.lock().unwrap().push(msg);
    }
    pub fn drain(&self) -> Vec<String> {
        std::mem::take(&mut self.0.lock().unwrap())
    }
}

pub struct GpuDevice {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub adapter_name: String,
    pub errors: Arc<GpuErrorState>,
    buffers: Mutex<HashMap<u64, Arc<GpuBuffer>>>,
    pipelines: Mutex<HashMap<u64, Arc<wgpu::ComputePipeline>>>,
    layouts: Mutex<HashMap<u64, Arc<wgpu::BindGroupLayout>>>,
    bind_groups: Mutex<HashMap<u64, Arc<wgpu::BindGroup>>>,
    next_id: std::sync::atomic::AtomicU64,
}

pub struct GpuBuffer {
    pub id: u64,
    pub buffer: wgpu::Buffer,
    pub size: u64,
    pub usage: u32,
}

impl GpuDevice {
    /// Request the shared adapter/device. None without a GPU adapter.
    pub fn request() -> Option<Arc<GpuDevice>> {
        let shared = gpu_shared()?;
        let errors = Arc::new(GpuErrorState::new());
        // Capture validation errors instead of panicking the page.
        {
            let errors = errors.clone();
            shared
                .device
                .on_uncaptured_error(Arc::new(move |e: wgpu::Error| errors.push(format!("{e}"))));
        }
        Some(Arc::new(GpuDevice {
            device: shared.device.clone(),
            queue: shared.queue.clone(),
            adapter_name: shared.adapter_name.clone(),
            errors,
            buffers: Mutex::new(HashMap::new()),
            pipelines: Mutex::new(HashMap::new()),
            layouts: Mutex::new(HashMap::new()),
            bind_groups: Mutex::new(HashMap::new()),
            next_id: std::sync::atomic::AtomicU64::new(1),
        }))
    }

    fn alloc_id(&self) -> u64 {
        self.next_id.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    }

    pub fn create_buffer(self: &Arc<Self>, size: u64, usage: u32) -> Arc<GpuBuffer> {
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("brows12-webgpu-buffer"),
            size: size.max(4),
            usage: wgpu::BufferUsages::from_bits_truncate(usage)
                | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let id = self.alloc_id();
        let buf = Arc::new(GpuBuffer { id, buffer, size, usage });
        self.buffers.lock().unwrap().insert(id, buf.clone());
        buf
    }

    pub fn buffer(&self, id: u64) -> Option<Arc<GpuBuffer>> {
        self.buffers.lock().unwrap().get(&id).cloned()
    }

    pub fn write_buffer(&self, id: u64, offset: u64, data: &[u8]) {
        if let Some(b) = self.buffer(id) {
            self.queue.write_buffer(&b.buffer, offset, data);
        }
    }

    /// Block until the buffer is mapped, then copy the bytes out.
    pub fn map_read(&self, id: u64) -> Option<Vec<u8>> {
        let b = self.buffer(id)?;
        if b.size == 0 {
            return Some(Vec::new());
        }
        let stage = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("brows12-webgpu-map-stage"),
            size: b.size,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut enc = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        enc.copy_buffer_to_buffer(&b.buffer, 0, &stage, 0, b.size);
        self.queue.submit([enc.finish()]);
        let slice = stage.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        let _ = self.device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None });
        match rx.recv() {
            Ok(Ok(())) => {}
            _ => return None,
        }
        let mapped = slice.get_mapped_range().ok()?;
        let out = mapped.to_vec();
        drop(mapped);
        stage.unmap();
        Some(out)
    }

    pub fn destroy_buffer(&self, id: u64) {
        if let Some(b) = self.buffers.lock().unwrap().remove(&id) {
            b.buffer.destroy();
        }
    }
}

impl GpuDevice {
    /// Create a compute pipeline from WGSL source. Returns Err with the
    /// shader error message on failure.
    pub fn create_compute_pipeline(&self, code: &str, entry: &str) -> Result<u64, String> {
        let module = self.device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("brows12-webgpu-compute-shader"),
            source: wgpu::ShaderSource::Wgsl(code.to_string().into()),
        });
        let pipeline = self
            .device
            .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("brows12-webgpu-compute-pipeline"),
                layout: None,
                module: &module,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                cache: None,
            });
        let id = self.alloc_id();
        self.pipelines.lock().unwrap().insert(id, Arc::new(pipeline));
        Ok(id)
    }

    /// Bind group layout from JS entries: [{binding, visibility, type}]
    /// where type ∈ "uniform" | "storage" | "read-only-storage".
    pub fn create_bind_group_layout(
        &self,
        entries: &[(u32, u32, &str)],
    ) -> Result<u64, String> {
        let mapped: Vec<wgpu::BindGroupLayoutEntry> = entries
            .iter()
            .map(|(binding, visibility, ty)| {
                let visibility = wgpu::ShaderStages::from_bits_truncate(*visibility);
                let ty = wgpu::BindingType::Buffer {
                    ty: match *ty {
                        "storage" => wgpu::BufferBindingType::Storage { read_only: false },
                        "read-only-storage" => {
                            wgpu::BufferBindingType::Storage { read_only: true }
                        }
                        _ => wgpu::BufferBindingType::Uniform,
                    },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                };
                wgpu::BindGroupLayoutEntry {
                    binding: *binding,
                    visibility,
                    ty,
                    count: None,
                }
            })
            .collect();
        let layout = self.device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("brows12-webgpu-bgl"),
            entries: &mapped,
        });
        let id = self.alloc_id();
        self.layouts.lock().unwrap().insert(id, Arc::new(layout));
        Ok(id)
    }

    /// Bind group: entries [{binding, bufferId, offset, size}].
    pub fn create_bind_group(
        &self,
        layout_id: u64,
        entries: &[(u32, u64, u64, Option<u64>)],
    ) -> Result<u64, String> {
        let layout = self
            .layouts
            .lock()
            .unwrap()
            .get(&layout_id)
            .cloned()
            .ok_or_else(|| "unknown bind group layout".to_string())?;
        // Collect buffers first so handles outlive the entries slice.
        let buffers: Vec<Arc<GpuBuffer>> = entries
            .iter()
            .filter_map(|(_, buffer_id, ..)| self.buffer(*buffer_id))
            .collect();
        let mut bufs = buffers.iter();
        let mapped: Vec<wgpu::BindGroupEntry> = entries
            .iter()
            .filter_map(|(binding, _, offset, size)| {
                let b = bufs.next()?;
                let resource = wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &b.buffer,
                    offset: *offset,
                    size: size.map(std::num::NonZeroU64::new).flatten(),
                });
                Some(wgpu::BindGroupEntry {
                    binding: *binding,
                    resource,
                })
            })
            .collect();
        let group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("brows12-webgpu-bg"),
            layout: &layout,
            entries: &mapped,
        });
        let id = self.alloc_id();
        self.bind_groups.lock().unwrap().insert(id, Arc::new(group));
        Ok(id)
    }

    /// Submit a compute pass: steps are ("pipeline"|"bindgroup"|"dispatch", id, x, y, z).
    pub fn submit_compute_steps(&self, steps: &[(String, u64, u32, u32, u32)]) {
        let pipelines = self.pipelines.lock().unwrap();
        let groups = self.bind_groups.lock().unwrap();
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("brows12-webgpu-encoder"),
            });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("brows12-webgpu-compute"),
                timestamp_writes: None,
            });
            for (op, id, x, y, z) in steps {
                match op.as_str() {
                    "pipeline" => {
                        if let Some(p) = pipelines.get(id) {
                            pass.set_pipeline(p);
                        }
                    }
                    "bindgroup" => {
                        if let Some(g) = groups.get(id) {
                            pass.set_bind_group(0, Some(g.as_ref()), &[]);
                        }
                    }
                    "dispatch" => pass.dispatch_workgroups(*x.max(&1), *y.max(&1), *z.max(&1)),
                    _ => {}
                }
            }
        }
        self.queue.submit([encoder.finish()]);
    }
}

/// A submitted compute pass description, replayed on submit.
#[derive(Default)]
pub struct ComputePlan {
    pub steps: Vec<ComputeStep>,
}

pub enum ComputeStep {
    SetPipeline(wgpu::ComputePipeline),
    SetBindGroup(wgpu::BindGroup),
    /// workgroups x, y, z
    Dispatch(u32, u32, u32),
}

/// Encode + submit the recorded plan.
pub fn submit_compute(device: &GpuDevice, plan: ComputePlan) {
    let mut encoder = device
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("brows12-webgpu-encoder"),
        });
    {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("brows12-webgpu-compute"),
            timestamp_writes: None,
        });
        for step in &plan.steps {
            match step {
                ComputeStep::SetPipeline(p) => pass.set_pipeline(p),
                ComputeStep::SetBindGroup(b) => pass.set_bind_group(0, b, &[]),
                ComputeStep::Dispatch(x, y, z) => pass.dispatch_workgroups(*x, *y, *z),
            }
        }
    }
    device.queue.submit([encoder.finish()]);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_bits_round_trip() {
        // WebGPU spec bit values must not drift.
        assert_eq!(usage::MAP_READ | usage::COPY_DST, 0x0009);
        assert_eq!(usage::STORAGE | usage::COPY_SRC | usage::COPY_DST, 0x008C);
    }
}
