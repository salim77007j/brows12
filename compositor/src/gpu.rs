//! GPU composite backend (wgpu 30): layer textures + transform/opacity
//! uniforms, offscreen render target, readback. Scrolling, transforms and
//! opacity never re-rasterize layer contents — that is the entire point.

use crate::layer::Layer;
use crate::{Backend, CompositeOutput, Compositor, CompositorError, CompositeStats};

/// WGSL: textured quad, premultiplied-alpha blending, per-layer affine
/// matrix + opacity uniform.
const SHADER: &str = r#"
struct LayerUniforms {
    matrix: mat3x3<f32>,
    opacity: f32,
    pad: vec3<f32>,
};
@group(0) @binding(0) var<uniform> u: LayerUniforms;
@group(0) @binding(1) var t: texture_2d<f32>;
@group(0) @binding(2) var s: sampler;

struct VSOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs(@builtin(vertex_index) vi: u32) -> VSOut {
    let quad = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(1.0, 1.0),
        vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 1.0), vec2<f32>(0.0, 1.0));
    let p = quad[vi];
    var out: VSOut;
    let tp = u.matrix * vec3<f32>(p, 1.0);
    out.pos = vec4<f32>(tp.x, tp.y, 0.0, 1.0);
    out.uv = vec2<f32>(p.x, 1.0 - p.y);
    return out;
}

@fragment
fn fs(in: VSOut) -> @location(0) vec4<f32> {
    let c = textureSample(t, s, in.uv);
    return vec4<f32>(c.rgb * u.opacity, c.a * u.opacity);
}
"#;

/// Per-layer uniforms: 3x3 column-major affine + opacity (vec4 padded).
#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct LayerUniforms {
    /// 3x3 column-major affine, columns padded to vec4 stride (WGSL uniform
    /// layout), then opacity + padding = 64 bytes total.
    m: [f32; 16],
}

/// wgpu-backed compositor.
pub struct GpuCompositor {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    adapter_name: String,
    adapter_api: String,
}

impl GpuCompositor {
    /// Request an adapter (any backend). Fails gracefully in GPU-less
    /// environments — callers fall back to `CpuCompositor`.
    pub fn new() -> Result<Self, CompositorError> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            compatible_surface: None,
            force_fallback_adapter: false,
            apply_limit_buckets: false,
        }))
        .map_err(|e| CompositorError::Gpu(format!("no adapter: {e}")))?;
        let info = adapter.get_info();
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("brows12-compositor"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::default(),
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
            memory_hints: wgpu::MemoryHints::Performance,
            trace: wgpu::Trace::Off,
        }))
        .map_err(|e| CompositorError::Gpu(format!("no device: {e}")))?;

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("brows12-composite-shader"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("brows12-layer-bind"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("brows12-pipeline-layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("brows12-composite-pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    blend: Some(wgpu::BlendState {
                        color: wgpu::BlendComponent {
                            src_factor: wgpu::BlendFactor::One,
                            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                            operation: wgpu::BlendOperation::Add,
                        },
                        alpha: wgpu::BlendComponent {
                            src_factor: wgpu::BlendFactor::One,
                            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                            operation: wgpu::BlendOperation::Add,
                        },
                    }),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        Ok(GpuCompositor {
            device,
            queue,
            pipeline,
            bind_group_layout,
            adapter_name: if info.name.is_empty() { "gpu-adapter".into() } else { info.name },
            adapter_api: format!("{:?}", info.backend),
        })
    }

    pub fn adapter_name(&self) -> &str {
        &self.adapter_name
    }

    /// Affine matrix mapping layer-local [0,1]² into target NDC (Y-up),
    /// including layer transform, placement offset and scroll offset.
    fn layer_matrix(layer: &Layer, target_w: f32, target_h: f32) -> LayerUniforms {
        let lw = layer.pixmap.width() as f32;
        let lh = layer.pixmap.height() as f32;
        let (ox, oy) = layer.effective_offset((0.0, 0.0));
        let t = &layer.transform;
        let rad = t.rotate_deg.to_radians();
        let (s, c) = (rad.sin(), rad.cos());
        let sc = t.scale;
        // local px -> rotate/scale -> translate(offset+scroll) -> NDC.
        let a = c * sc * lw;
        let b = s * sc * lw;
        let cc = -s * sc * lh;
        let d = c * sc * lh;
        let e = t.tx + ox;
        let f = t.ty + oy;
        let m00 = a / target_w * 2.0;
        let m01 = cc / target_w * 2.0;
        let m03 = (e / target_w * 2.0) - 1.0;
        let m10 = b / target_h * -2.0;
        let m11 = d / target_h * -2.0;
        let m13 = 1.0 - (f / target_h * 2.0);
        LayerUniforms {
            m: [
                m00, m10, 0.0, 0.0, // column 0 (x basis)
                m01, m11, 0.0, 0.0, // column 1 (y basis)
                m03, m13, 0.0, 0.0, // column 2 (translation)
                layer.opacity.clamp(0.0, 1.0),
                0.0,
                0.0,
                0.0,
            ],
        }
    }

    fn draw_layers(
        &self,
        width: u32,
        height: u32,
        layers: &[Layer],
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
    ) -> Result<(), CompositorError> {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("brows12-composite-pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&self.pipeline);
        let sampler = self.device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("brows12-layer-sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        for layer in layers {
            let lw = layer.pixmap.width();
            let lh = layer.pixmap.height();
            if lw == 0 || lh == 0 {
                continue;
            }
            let texture = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("brows12-layer-texture"),
                size: wgpu::Extent3d { width: lw, height: lh, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            self.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                layer.pixmap.data(),
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(lw * 4),
                    rows_per_image: Some(lh),
                },
                wgpu::Extent3d { width: lw, height: lh, depth_or_array_layers: 1 },
            );
            let view_t = texture.create_view(&wgpu::TextureViewDescriptor::default());
            let uniforms = Self::layer_matrix(layer, width as f32, height as f32);
            use wgpu::util::DeviceExt as _;
            let ubuf = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("brows12-layer-uniform"),
                contents: bytemuck::bytes_of(&uniforms),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            });
            let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("brows12-layer-bind-group"),
                layout: &self.bind_group_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: ubuf.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&view_t),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::Sampler(&sampler),
                    },
                ],
            });
            pass.set_bind_group(0, &bind, &[]);
            pass.draw(0..6, 0..1);
        }
        Ok(())
    }
}

impl Compositor for GpuCompositor {
    fn composite(
        &self,
        width: u32,
        height: u32,
        layers: &[Layer],
    ) -> Result<CompositeOutput, CompositorError> {
        let t0 = std::time::Instant::now();
        if layers.is_empty() {
            return Err(CompositorError::Empty);
        }
        let target = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("brows12-composite-target"),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = target.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("brows12-composite-encoder"),
            });
        self.draw_layers(width, height, layers, &mut encoder, &view)?;

        // Readback: 256-aligned staging buffer, map, de-pad rows.
        let bytes_per_row = ((width * 4 + 255) / 256) * 256;
        let stage_size = (bytes_per_row * height) as u64;
        let stage = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("brows12-readback"),
            size: stage_size,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &target,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &stage,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(bytes_per_row),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        );
        self.queue.submit([encoder.finish()]);

        let slice = stage.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        // Wait for completion; ignore backend-specific poll statuses.
        let _ = self.device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None });
        rx.recv()
            .map_err(|_| CompositorError::Gpu("readback channel closed".into()))?
            .map_err(|e| CompositorError::Gpu(format!("map failed: {e}")))?;
        let mapped: wgpu::BufferView =
            slice.get_mapped_range().map_err(|e| CompositorError::Gpu(format!("view: {e}")))?;
        let mut pixels = Vec::with_capacity((width * height * 4) as usize);
        for row in 0..height {
            let start = (row * bytes_per_row) as usize;
            pixels.extend_from_slice(&mapped[start..start + (width * 4) as usize]);
        }
        drop(mapped);
        stage.unmap();

        // Premultiplied -> straight alpha for the shared output contract.
        let straight = crate::cpu::premul_to_straight(&pixels);
        let stats = CompositeStats {
            backend: Backend::Gpu {
                adapter: self.adapter_name.clone(),
                api: self.adapter_api.clone(),
            },
            frame_time_us: t0.elapsed().as_secs_f64() * 1e6,
            layer_count: layers.len(),
            texture_bytes: layers.iter().map(Layer::texture_bytes).sum(),
        };
        Ok(CompositeOutput { width, height, pixels: straight, stats })
    }
}
