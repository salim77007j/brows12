//! WebGL 2 for page scripts, over the engine's wgpu 30 stack.
//!
//! Architecture (honest scope, see docs/CAPABILITY_REPORT.md §Graphics):
//! - One lazily-created shared `wgpu` device for all WebGL surfaces.
//! - Per-canvas offscreen RGBA8 color target (+ optional Depth24Plus).
//! - GLSL ES 1.00/3.00 shaders are normalized ([`crate::glsl`]) → parsed and
//!   validated by naga (desktop 460 core) → emitted as WGSL → wgpu pipelines.
//! - The GL state machine (buffers, VAOs, textures, programs, uniforms,
//!   blend/depth/cull) is implemented engine-side; draw commands execute as
//!   one wgpu render pass over the canvas target.
//! - Pixels leave the GPU on readback/harvest: staging copy → straight alpha
//!   → engine image pipeline (same contract as Canvas2D surfaces).
//!
//! Supported: shaders/programs/buffers/VAOs/textures/uniforms
//! (mat, vec, scalar, f32+int), drawArrays/drawElements (TRIANGLES, STRIP,
//! LINES, POINTS), blending, depth test, culling, scissor, viewport,
//! clearColor/clearDepth, readPixels. Not supported yet (documented): FBOs,
//! transform feedback, queries, instancing, TRIANGLE_FAN/LINE_LOOP.

use crate::glsl::{self, BindingMap, Stage};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

// ---------------------------------------------------------------------------
// Shared GPU context
// ---------------------------------------------------------------------------

pub struct GpuShared {
    pub adapter_name: String,
    pub backend: String,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
}

static GPU: OnceLock<Option<Arc<GpuShared>>> = OnceLock::new();

/// Shared adapter/device/queue. `None` in GPU-less environments (headless
/// containers without lavapipe) — callers must treat that like a browser
/// without WebGL support (getContext returns null).
pub fn gpu_shared() -> Option<Arc<GpuShared>> {
    GPU.get_or_init(|| {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: None,
            force_fallback_adapter: false,
            apply_limit_buckets: false,
        }))
        .ok()?;
        let info = adapter.get_info();
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("brows12-webgl"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::default(),
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
            memory_hints: wgpu::MemoryHints::Performance,
            trace: wgpu::Trace::Off,
        }))
        .ok()?;
        let name = if info.name.is_empty() { "gpu-adapter".into() } else { info.name };
        Some(Arc::new(GpuShared {
            adapter_name: name,
            backend: format!("{:?}", info.backend),
            device,
            queue,
        }))
    })
    .clone()
}

/// Fast probe for tests/CI: is a GPU adapter available in this environment?
pub fn gpu_available() -> bool {
    gpu_shared().is_some()
}

// ---------------------------------------------------------------------------
// GL objects (engine-side state machine)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct GlBuffer {
    pub data: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct GlShader {
    pub stage: Stage,
    pub source: String,
    /// WGSL after normalization + naga parse/validate; Err carries the
    /// compiler message surfaced via getShaderInfoLog.
    pub compiled: Result<CompiledShader, String>,
}

#[derive(Debug, Clone)]
pub struct CompiledShader {
    pub wgsl: String,
    /// Uniform (UBO) bindings: binding -> (name, byte size, is_int).
    pub uniforms: HashMap<u32, (String, u32, bool)>,
    /// Texture bindings: binding -> sampler name.
    pub textures: HashMap<u32, String>,
    pub entry: String,
}

#[derive(Debug, Clone)]
pub struct GlTexture {
    pub width: u32,
    pub height: u32,
    /// RGBA8 straight-alpha bytes (top-left origin).
    pub pixels: Vec<u8>,
    pub min_filter: u32,
    pub mag_filter: u32,
    pub wrap_s: u32,
    pub wrap_t: u32,
}

#[derive(Debug, Clone)]
pub struct GlAttrib {
    pub buffer: u32,
    pub size: i32,
    pub normalized: bool,
    pub stride: i32,
    pub offset: u32,
    pub typ: u32, // FLOAT or UNSIGNED_BYTE
}

#[derive(Debug, Clone, Default)]
pub struct GlVertexArray {
    pub attribs: HashMap<u32, GlAttrib>,
    pub element_buffer: u32,
}

#[derive(Debug, Default)]
pub struct GlProgram {
    vs: u32,
    fs: u32,
    linked: Option<LinkedProgram>,
}

#[derive(Debug)]
pub struct LinkedProgram {
    pub vs: CompiledShader,
    pub fs: CompiledShader,
    /// binding -> CPU shadow bytes (uploaded to a device buffer per draw).
    pub uniform_shadows: HashMap<u32, Vec<u8>>,
    pub uniform_kinds: HashMap<u32, (u32, bool)>, // binding -> (size, is_int)
    /// Vertex attribute name -> WebGL location (the injected naga location).
    pub attributes: std::collections::BTreeMap<String, u32>,
}

#[derive(Default)]
struct Objects {
    next_id: u32,
    buffers: HashMap<u32, GlBuffer>,
    shaders: HashMap<u32, GlShader>,
    programs: HashMap<u32, GlProgram>,
    textures: HashMap<u32, GlTexture>,
    vaos: HashMap<u32, GlVertexArray>,
}

impl Objects {
    fn alloc(&mut self) -> u32 {
        self.next_id += 1;
        self.next_id
    }
}

// GL constants exposed to JS (values match the WebGL spec).
pub mod gl {
    pub const FLOAT: u32 = 0x1406;
    pub const UNSIGNED_BYTE: u32 = 0x1401;
    pub const UNSIGNED_SHORT: u32 = 0x1403;
    pub const UNSIGNED_INT: u32 = 0x1405;
    pub const ARRAY_BUFFER: u32 = 0x8892;
    pub const ELEMENT_ARRAY_BUFFER: u32 = 0x8893;
    pub const STATIC_DRAW: u32 = 0x88E4;
    pub const VERTEX_SHADER: u32 = 0x8B31;
    pub const FRAGMENT_SHADER: u32 = 0x8B30;
    pub const COMPILE_STATUS: u32 = 0x8B81;
    pub const LINK_STATUS: u32 = 0x8B82;
    pub const TEXTURE_2D: u32 = 0x0DE1;
    pub const TEXTURE0: u32 = 0x84C0;
    pub const RGBA: u32 = 0x1908;
    pub const RGB: u32 = 0x1907;
    pub const TEXTURE_MIN_FILTER: u32 = 0x2801;
    pub const TEXTURE_MAG_FILTER: u32 = 0x2800;
    pub const TEXTURE_WRAP_S: u32 = 0x2802;
    pub const TEXTURE_WRAP_T: u32 = 0x2803;
    pub const LINEAR: u32 = 0x2601;
    pub const NEAREST: u32 = 0x2600;
    pub const REPEAT: u32 = 0x2901;
    pub const CLAMP_TO_EDGE: u32 = 0x812F;
    pub const COLOR_BUFFER_BIT: u32 = 0x4000;
    pub const DEPTH_BUFFER_BIT: u32 = 0x100;
    pub const DEPTH_TEST: u32 = 0x0B71;
    pub const BLEND: u32 = 0x0BE2;
    pub const CULL_FACE: u32 = 0x0B44;
    pub const SCISSOR_TEST: u32 = 0x0C11;
    pub const LESS: u32 = 0x201;
    pub const LEQUAL: u32 = 0x203;
    pub const GREATER: u32 = 0x204;
    pub const GEQUAL: u32 = 0x206;
    pub const EQUAL: u32 = 0x202;
    pub const NOTEQUAL: u32 = 0x205;
    pub const ALWAYS: u32 = 0x207;
    pub const NEVER: u32 = 0x200;
    pub const ZERO: u32 = 0;
    pub const ONE: u32 = 1;
    pub const SRC_ALPHA: u32 = 0x0302;
    pub const ONE_MINUS_SRC_ALPHA: u32 = 0x0303;
    pub const SRC_COLOR: u32 = 0x0300;
    pub const ONE_MINUS_SRC_COLOR: u32 = 0x0301;
    pub const DST_ALPHA: u32 = 0x0304;
    pub const ONE_MINUS_DST_ALPHA: u32 = 0x0305;
    pub const DST_COLOR: u32 = 0x0306;
    pub const ONE_MINUS_DST_COLOR: u32 = 0x0307;
    pub const CCW: u32 = 0x0901;
    pub const CW: u32 = 0x0900;
    pub const BACK: u32 = 0x0405;
    pub const FRONT: u32 = 0x0404;
    pub const FRONT_AND_BACK: u32 = 0x0408;
    pub const TRIANGLES: u32 = 4;
    pub const TRIANGLE_STRIP: u32 = 5;
    pub const TRIANGLE_FAN: u32 = 6;
    pub const LINES: u32 = 1;
    pub const LINE_STRIP: u32 = 3;
    pub const LINE_LOOP: u32 = 2;
    pub const POINTS: u32 = 0;
    pub const NO_ERROR: u32 = 0;
    pub const INVALID_ENUM: u32 = 0x0500;
    pub const INVALID_VALUE: u32 = 0x0501;
    pub const INVALID_OPERATION: u32 = 0x0502;
    pub const INVALID_FRAMEBUFFER_OPERATION: u32 = 0x0506;
    pub const OUT_OF_MEMORY: u32 = 0x0505;
    pub const CONTEXT_LOST_WEBGL: u32 = 0x9242;
}

// ---------------------------------------------------------------------------
// Per-canvas surface
// ---------------------------------------------------------------------------

/// A WebGL canvas surface: color target + full GL state.
pub struct WebGlSurface {
    pub node_id: i32,
    pub width: u32,
    pub height: u32,
    gpu: Arc<GpuShared>,
    color: wgpu::Texture,
    depth: Option<wgpu::Texture>,
    objects: Objects,
    st: GlRuntimeState,
    pipelines: HashMap<u64, wgpu::RenderPipeline>,
    /// (program, uniform binding) -> (device uniform buffer, byte capacity).
    uniform_bufs: HashMap<(u32, u32), (wgpu::Buffer, u64)>,
}

#[derive(Default, Clone)]
struct GlRuntimeState {
    clear_color: [f64; 4],
    clear_depth: f64,
    viewport: [i32; 4],
    blend: bool,
    depth_test: bool,
    cull: bool,
    scissor: Option<[i32; 4]>,
    blend_src_rgb: u32,
    blend_dst_rgb: u32,
    blend_src_a: u32,
    blend_dst_a: u32,
    depth_func: u32,
    depth_mask: bool,
    front_face: u32,
    cull_face: u32,
    array_buffer: u32,
    element_buffer: u32,
    program: u32,
    vao: u32,
    textures: [u32; 16],
    active_unit: usize,
    default_vao: GlVertexArray,
    attrib_enabled: [bool; 16],
    pending_clear: bool,
    flip_y: bool,
    last_error: u32,
}

impl GlRuntimeState {
    fn new(width: u32, height: u32) -> Self {
        GlRuntimeState {
            clear_depth: 1.0,
            viewport: [0, 0, width as i32, height as i32],
            blend_src_rgb: gl::ONE,
            blend_dst_rgb: gl::ZERO,
            blend_src_a: gl::ONE,
            blend_dst_a: gl::ZERO,
            depth_func: gl::LESS,
            depth_mask: true,
            front_face: gl::CCW,
            cull_face: gl::BACK,
            ..Default::default()
        }
    }
}

impl WebGlSurface {
    fn new(node_id: i32, width: u32, height: u32, gpu: Arc<GpuShared>) -> Self {
        let w = width.max(1);
        let h = height.max(1);
        let color = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("brows12-webgl-canvas"),
            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        WebGlSurface {
            node_id,
            width: w,
            height: h,
            gpu,
            color,
            depth: None,
            objects: Objects::default(),
            st: GlRuntimeState::new(w, h),
            pipelines: HashMap::new(),
            uniform_bufs: HashMap::new(),
        }
    }

    /// Restore target after canvas resize (spec: resets the drawing buffer).
    pub fn resize(&mut self, width: u32, height: u32) {
        let (w, h) = (width.max(1), height.max(1));
        if w == self.width && h == self.height {
            return;
        }
        self.width = w;
        self.height = h;
        self.color = self.gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("brows12-webgl-canvas"),
            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        self.depth = None;
        self.st.viewport = [0, 0, w as i32, h as i32];
        self.pipelines.clear();
    }
}

// ---------------------------------------------------------------------------
// Store (one realm's WebGL canvases)
// ---------------------------------------------------------------------------

/// Registry of WebGL surfaces for one page realm (keyed by DOM node id).
#[derive(Default)]
pub struct WebGlStore {
    surfaces: Mutex<HashMap<i32, Arc<Mutex<WebGlSurface>>>>,
}

impl WebGlStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Create (or resize) the WebGL context for a canvas. `None` when no GPU
    /// adapter exists — the JS side then returns `null` from getContext.
    pub fn get_context(
        &self,
        node_id: i32,
        width: u32,
        height: u32,
    ) -> Option<(Arc<Mutex<WebGlSurface>>, String, String)> {
        let gpu = gpu_shared()?;
        let mut map = self.surfaces.lock().unwrap();
        let surface = map.entry(node_id).or_insert_with(|| {
            Arc::new(Mutex::new(WebGlSurface::new(node_id, width, height, gpu.clone())))
        });
        {
            let mut s = surface.lock().unwrap();
            s.resize(width, height);
        }
        let name = gpu.adapter_name.clone();
        let backend = gpu.backend.clone();
        Some((surface.clone(), name, backend))
    }

    pub fn clear(&self) {
        self.surfaces.lock().unwrap().clear();
    }

    /// Shared handle for the bindings layer (None → JS never sees a context).
    pub fn surface(&self, node_id: i32) -> Option<Arc<Mutex<WebGlSurface>>> {
        self.surfaces.lock().unwrap().get(&node_id).cloned()
    }

    /// Read back every surface for engine harvesting:
    /// (node_id, width, height, straight-alpha RGBA pixels).
    pub fn harvest(&self) -> Vec<(i32, u32, u32, Vec<u8>)> {
        let mut out = Vec::new();
        let map = self.surfaces.lock().unwrap();
        for (id, surface) in map.iter() {
            let mut s = surface.lock().unwrap();
            if let Some(pixels) = s.readback() {
                out.push((*id, s.width, s.height, pixels));
            }
        }
        out
    }
}

// ---------------------------------------------------------------------------
// Shader compilation: GLSL ES → naga → WGSL
// ---------------------------------------------------------------------------

fn naga_stage(stage: Stage) -> naga::ShaderStage {
    match stage {
        Stage::Vertex => naga::ShaderStage::Vertex,
        Stage::Fragment => naga::ShaderStage::Fragment,
    }
}

/// Parse + validate via naga, extract resource metadata, emit WGSL.
fn compile_shader(src: &str, stage: Stage, map: &BindingMap) -> Result<CompiledShader, String> {
    let normalized = glsl::normalize(src, stage, map)?;
    let mut fe = naga::front::glsl::Frontend::default();
    let opts = naga::front::glsl::Options::from(naga_stage(stage));
    let module = fe.parse(&opts, &normalized).map_err(|e| e.to_string())?;
    let mut validator = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    );
    let info = validator.validate(&module).map_err(|e| e.to_string())?;

    let mut uniforms = HashMap::new();
    let mut textures = HashMap::new();
    for (_handle, gv) in module.global_variables.iter() {
        let Some(binding) = gv.binding.as_ref() else { continue };
        if binding.group != 0 {
            continue;
        }
        let ty = &module.types[gv.ty];
        match (&gv.space, &ty.inner) {
            (naga::AddressSpace::Uniform, inner) => {
                let size = std140_size(inner);
                let is_int = matches!(
                    inner.scalar_kind(),
                    Some(naga::ScalarKind::Sint) | Some(naga::ScalarKind::Uint)
                );
                uniforms
                    .insert(binding.binding, (gv.name.clone().unwrap_or_default(), size, is_int));
            }
            (naga::AddressSpace::Handle, naga::TypeInner::Image { .. }) => {
                textures.insert(binding.binding, gv.name.clone().unwrap_or_default());
            }
            _ => {}
        }
    }
    let entry =
        module.entry_points.first().map(|e| e.name.clone()).unwrap_or_else(|| "main".to_string());

    let mut out = String::new();
    let mut writer =
        naga::back::wgsl::Writer::new(&mut out, naga::back::wgsl::WriterFlags::empty());
    writer.write(&module, &info).map_err(|e| e.to_string())?;
    Ok(CompiledShader { wgsl: out, uniforms, textures, entry })
}

// ---------------------------------------------------------------------------
// GL operations (called from JS bindings)
// ---------------------------------------------------------------------------

fn err(st: &mut GlRuntimeState, code: u32) {
    if st.last_error == gl::NO_ERROR {
        st.last_error = code;
    }
}

impl WebGlSurface {
    // -- object creation ----------------------------------------------------

    pub fn create_buffer(&mut self) -> u32 {
        let id = self.objects.alloc();
        self.objects.buffers.insert(id, GlBuffer { data: Vec::new() });
        id
    }
    pub fn delete_buffer(&mut self, id: u32) {
        self.objects.buffers.remove(&id);
    }
    pub fn create_shader(&mut self, stage: u32) -> u32 {
        let id = self.objects.alloc();
        let st = if stage == gl::VERTEX_SHADER { Stage::Vertex } else { Stage::Fragment };
        self.objects.shaders.insert(
            id,
            GlShader { stage: st, source: String::new(), compiled: Err("not compiled".into()) },
        );
        id
    }
    pub fn delete_shader(&mut self, id: u32) {
        self.objects.shaders.remove(&id);
    }
    pub fn create_program(&mut self) -> u32 {
        let id = self.objects.alloc();
        self.objects.programs.insert(id, GlProgram::default());
        id
    }
    pub fn delete_program(&mut self, id: u32) {
        self.objects.programs.remove(&id);
    }
    pub fn create_texture(&mut self) -> u32 {
        let id = self.objects.alloc();
        self.objects.textures.insert(
            id,
            GlTexture {
                width: 0,
                height: 0,
                pixels: Vec::new(),
                min_filter: gl::NEAREST,
                mag_filter: gl::NEAREST,
                wrap_s: gl::REPEAT,
                wrap_t: gl::REPEAT,
            },
        );
        id
    }
    pub fn delete_texture(&mut self, id: u32) {
        self.objects.textures.remove(&id);
    }
    pub fn create_vertex_array(&mut self) -> u32 {
        let id = self.objects.alloc();
        self.objects.vaos.insert(id, GlVertexArray::default());
        id
    }
    pub fn delete_vertex_array(&mut self, id: u32) {
        self.objects.vaos.remove(&id);
        if self.st.vao == id {
            self.st.vao = 0;
        }
    }

    // -- shaders / programs --------------------------------------------------

    pub fn shader_source(&mut self, id: u32, src: String) {
        if let Some(sh) = self.objects.shaders.get_mut(&id) {
            sh.source = src;
        }
    }
    pub fn compile_shader(&mut self, id: u32) {
        let map_ok = {
            let Some(sh) = self.objects.shaders.get(&id) else { return };
            !sh.source.is_empty()
        };
        if !map_ok {
            return;
        }
        let (stage, source) = {
            let sh = &self.objects.shaders[&id];
            (sh.stage, sh.source.clone())
        };
        // Bindings come from the program's link step, but compile needs the
        // map too; use a program-wide map captured at link. For compile-time
        // we assign from a map built over BOTH stages via link-time reuse:
        // store per-shader map derived now and rebuilt at link.
        let map = self.binding_map_for(stage, &source);
        let compiled = compile_shader(&source, stage, &map);
        if let Some(sh) = self.objects.shaders.get_mut(&id) {
            sh.compiled = compiled;
        }
    }

    /// Build a binding map for one shader: scan its own source. Link then
    /// re-normalizes with the merged map (see link_program).
    fn binding_map_for(&self, stage: Stage, src: &str) -> BindingMap {
        let other = String::new();
        let (a, b): (&str, &str) =
            if stage == Stage::Vertex { (src, other.as_str()) } else { (other.as_str(), src) };
        glsl::collect_bindings(a, b).unwrap_or_default()
    }

    pub fn get_shader_compile_ok(&self, id: u32) -> Option<bool> {
        self.objects.shaders.get(&id).map(|s| s.compiled.is_ok())
    }
    pub fn get_shader_info_log(&self, id: u32) -> String {
        match self.objects.shaders.get(&id) {
            Some(s) => match &s.compiled {
                Ok(_) => String::new(),
                Err(e) => e.clone(),
            },
            None => "shader not found".into(),
        }
    }
    pub fn attach_shader(&mut self, prog: u32, shader: u32) {
        if let Some(p) = self.objects.programs.get_mut(&prog) {
            if let Some(sh) = self.objects.shaders.get(&shader) {
                match sh.stage {
                    Stage::Vertex => p.vs = shader,
                    Stage::Fragment => p.fs = shader,
                }
            }
        }
    }

    /// Link VS+FS: merge binding maps across both stages, recompile both with
    /// the merged map (so a name shares one binding in both stages), validate
    /// the pair can form one pipeline layout.
    pub fn link_program(&mut self, prog: u32) {
        let (vs_id, fs_id) = match self.objects.programs.get(&prog) {
            Some(p) => (p.vs, p.fs),
            None => return,
        };
        let (vs_src, fs_src) = match (
            self.objects.shaders.get(&vs_id).map(|s| s.source.clone()),
            self.objects.shaders.get(&fs_id).map(|s| s.source.clone()),
        ) {
            (Some(v), Some(f)) => (v, f),
            _ => return,
        };
        let map = match glsl::collect_bindings(&vs_src, &fs_src) {
            Ok(m) => m,
            Err(e) => {
                if let Some(p) = self.objects.programs.get_mut(&prog) {
                    p.linked = None;
                }
                // Record the failure on the VS for getProgramInfoLog.
                if let Some(sh) = self.objects.shaders.get_mut(&vs_id) {
                    sh.compiled = Err(e.clone());
                }
                let _ = e;
                return;
            }
        };
        let vs = compile_shader(&vs_src, Stage::Vertex, &map);
        let fs = compile_shader(&fs_src, Stage::Fragment, &map);
        let (vs_c, fs_c) = match (vs, fs) {
            (Ok(v), Ok(f)) => (v, f),
            (Err(e), _) | (_, Err(e)) => {
                if let Some(p) = self.objects.programs.get_mut(&prog) {
                    p.linked = None;
                }
                let _ = e;
                return;
            }
        };
        let mut uniform_shadows = HashMap::new();
        let mut uniform_kinds = HashMap::new();
        for (b, (_name, size, is_int)) in vs_c.uniforms.iter().chain(fs_c.uniforms.iter()) {
            uniform_shadows.entry(*b).or_insert_with(|| vec![0u8; *size as usize]);
            uniform_kinds.insert(*b, (*size, *is_int));
        }
        if let Some(p) = self.objects.programs.get_mut(&prog) {
            p.linked = Some(LinkedProgram {
                vs: vs_c,
                fs: fs_c,
                uniform_shadows,
                uniform_kinds,
                attributes: map.attributes,
            });
        }
    }
    pub fn get_program_link_ok(&self, prog: u32) -> Option<bool> {
        self.objects.programs.get(&prog).map(|p| p.linked.is_some())
    }
    pub fn get_program_info_log(&self, prog: u32) -> String {
        self.objects
            .programs
            .get(&prog)
            .map(|p| {
                if p.linked.is_some() {
                    String::new()
                } else {
                    "link failed (see shader logs)".into()
                }
            })
            .unwrap_or_else(|| "program not found".into())
    }

    // -- uniforms -------------------------------------------------------------

    /// getUniformLocation: return the binding number as the location id.
    pub fn get_uniform_location(&self, prog: u32, name: &str) -> Option<i32> {
        let p = self.objects.programs.get(&prog)?;
        let linked = p.linked.as_ref()?;
        for (b, (n, _, _)) in &linked.vs.uniforms {
            if n == name {
                return Some(*b as i32);
            }
        }
        for (b, (n, _, _)) in &linked.fs.uniforms {
            if n == name {
                return Some(*b as i32);
            }
        }
        None
    }

    fn uniform_write(&mut self, loc: i32, bytes: &[u8]) {
        let Some(p) = self.objects.programs.get_mut(&self.st.program) else { return };
        let Some(linked) = p.linked.as_mut() else { return };
        if let Some(shadow) = linked.uniform_shadows.get_mut(&(loc as u32)) {
            let n = bytes.len().min(shadow.len());
            shadow[..n].copy_from_slice(&bytes[..n]);
        }
    }

    pub fn uniform_1f(&mut self, loc: i32, x: f64) {
        self.uniform_write(loc, &(x as f32).to_le_bytes());
    }
    pub fn uniform_2f(&mut self, loc: i32, x: f64, y: f64) {
        self.uniform_write(loc, &[(x as f32).to_le_bytes(), (y as f32).to_le_bytes()].concat());
    }
    pub fn uniform_3f(&mut self, loc: i32, x: f64, y: f64, z: f64) {
        self.uniform_write(
            loc,
            &[(x as f32).to_le_bytes(), (y as f32).to_le_bytes(), (z as f32).to_le_bytes()]
                .concat(),
        );
    }
    pub fn uniform_4f(&mut self, loc: i32, x: f64, y: f64, z: f64, w: f64) {
        self.uniform_write(
            loc,
            &[
                (x as f32).to_le_bytes(),
                (y as f32).to_le_bytes(),
                (z as f32).to_le_bytes(),
                (w as f32).to_le_bytes(),
            ]
            .concat(),
        );
    }
    pub fn uniform_1i(&mut self, loc: i32, x: i32) {
        self.uniform_write(loc, &x.to_le_bytes());
    }
    /// uniformNfv / uniformMatrixNfv: column-major f32 payload.
    pub fn uniform_fv(&mut self, loc: i32, data: &[f32]) {
        let mut bytes = Vec::with_capacity(data.len() * 4);
        for v in data {
            bytes.extend_from_slice(&v.to_le_bytes());
        }
        self.uniform_write(loc, &bytes);
    }

    // -- buffers & VAOs -------------------------------------------------------

    pub fn bind_buffer(&mut self, target: u32, id: u32) {
        match target {
            gl::ARRAY_BUFFER => self.st.array_buffer = id,
            gl::ELEMENT_ARRAY_BUFFER => {
                if self.st.vao != 0 {
                    if let Some(vao) = self.objects.vaos.get_mut(&self.st.vao) {
                        vao.element_buffer = id;
                    }
                }
                self.st.element_buffer = id;
            }
            _ => err(&mut self.st, gl::INVALID_ENUM),
        }
    }

    pub fn buffer_data(&mut self, _target: u32, data: &[u8]) {
        if let Some(b) = self.objects.buffers.get_mut(&self.st.array_buffer) {
            b.data = data.to_vec();
        } else {
            err(&mut self.st, gl::INVALID_OPERATION);
        }
    }
    pub fn buffer_sub_data(&mut self, _target: u32, offset: u32, data: &[u8]) {
        if let Some(b) = self.objects.buffers.get_mut(&self.st.array_buffer) {
            let start = offset as usize;
            if start + data.len() > b.data.len() {
                err(&mut self.st, gl::INVALID_VALUE);
                return;
            }
            b.data[start..start + data.len()].copy_from_slice(data);
        } else {
            err(&mut self.st, gl::INVALID_OPERATION);
        }
    }

    pub fn bind_vertex_array(&mut self, id: u32) {
        self.st.vao = id;
    }

    pub fn enable_vertex_attrib_array(&mut self, idx: u32) {
        if (idx as usize) < self.st.attrib_enabled.len() {
            self.st.attrib_enabled[idx as usize] = true;
        }
    }
    pub fn disable_vertex_attrib_array(&mut self, idx: u32) {
        if (idx as usize) < self.st.attrib_enabled.len() {
            self.st.attrib_enabled[idx as usize] = false;
        }
    }

    /// Captures the CURRENT ARRAY_BUFFER binding (GL semantics).
    pub fn vertex_attrib_pointer(
        &mut self,
        idx: u32,
        size: i32,
        typ: u32,
        normalized: bool,
        stride: i32,
        offset: u32,
    ) {
        let buf = self.st.array_buffer;
        let attrib = GlAttrib { buffer: buf, size, normalized, stride, offset, typ };
        if self.st.vao != 0 {
            if let Some(vao) = self.objects.vaos.get_mut(&self.st.vao) {
                vao.attribs.insert(idx, attrib.clone());
            }
        }
        // Also record in the default VAO state so plain usage works.
        self.st.default_vao.attribs.insert(idx, attrib);
    }

    // -- textures --------------------------------------------------------------

    pub fn active_texture(&mut self, unit: u32) {
        if (gl::TEXTURE0..gl::TEXTURE0 + 16).contains(&unit) {
            self.st.active_unit = (unit - gl::TEXTURE0) as usize;
        } else {
            err(&mut self.st, gl::INVALID_ENUM);
        }
    }
    pub fn bind_texture(&mut self, _target: u32, id: u32) {
        self.st.textures[self.st.active_unit] = id;
    }
    pub fn tex_parameteri(&mut self, _target: u32, name: u32, value: u32) {
        if let Some(t) = self.objects.textures.get_mut(&self.st.textures[self.st.active_unit]) {
            match name {
                gl::TEXTURE_MIN_FILTER => t.min_filter = value,
                gl::TEXTURE_MAG_FILTER => t.mag_filter = value,
                gl::TEXTURE_WRAP_S => t.wrap_s = value,
                gl::TEXTURE_WRAP_T => t.wrap_t = value,
                _ => err(&mut self.st, gl::INVALID_ENUM),
            }
        }
    }

    /// texImage2D with raw RGBA8 (or RGB→RGBA-expanded) bytes.
    pub fn tex_image_2d_bytes(&mut self, width: u32, height: u32, pixels: &[u8], rgb: bool) {
        let mut data = pixels.to_vec();
        if rgb {
            let n = (width * height) as usize;
            let mut rgba = Vec::with_capacity(n * 4);
            for px in data.chunks_exact(3).take(n) {
                rgba.extend_from_slice(&[px[0], px[1], px[2], 255]);
            }
            data = rgba;
        }
        if let Some(t) = self.objects.textures.get_mut(&self.st.textures[self.st.active_unit]) {
            t.width = width;
            t.height = height;
            t.pixels = data;
        }
    }

    /// texImage2D from a Canvas2D surface (straight-alpha RGBA8).
    pub fn tex_image_2d_canvas(&mut self, width: u32, height: u32, rgba: &[u8]) {
        self.tex_image_2d_bytes(width, height, rgba, false);
    }

    pub fn pixel_storei_flip_y(&mut self, flip: bool) {
        self.st.flip_y = flip;
    }

    /// Apply UNPACK_FLIP_Y_WEBGL to an upload (row reversal).
    pub fn flip_rows(&self, w: u32, h: u32, data: &[u8]) -> Vec<u8> {
        let stride = (w * 4) as usize;
        let mut out = data.to_vec();
        for row in 0..h as usize {
            let src = (h as usize - 1 - row) * stride;
            out[row * stride..(row + 1) * stride].copy_from_slice(&data[src..src + stride]);
        }
        out
    }

    // -- state ------------------------------------------------------------------

    pub fn enable(&mut self, cap: u32) {
        match cap {
            gl::BLEND => self.st.blend = true,
            gl::DEPTH_TEST => {
                self.st.depth_test = true;
                if self.depth.is_none() {
                    self.depth = Some(self.gpu.device.create_texture(&wgpu::TextureDescriptor {
                        label: Some("brows12-webgl-depth"),
                        size: wgpu::Extent3d {
                            width: self.width,
                            height: self.height,
                            depth_or_array_layers: 1,
                        },
                        mip_level_count: 1,
                        sample_count: 1,
                        dimension: wgpu::TextureDimension::D2,
                        format: wgpu::TextureFormat::Depth24Plus,
                        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                        view_formats: &[],
                    }));
                }
            }
            gl::CULL_FACE => self.st.cull = true,
            gl::SCISSOR_TEST => {
                self.st.scissor = Some([0, 0, self.width as i32, self.height as i32])
            }
            _ => {}
        }
    }
    pub fn disable(&mut self, cap: u32) {
        match cap {
            gl::BLEND => self.st.blend = false,
            gl::DEPTH_TEST => self.st.depth_test = false,
            gl::CULL_FACE => self.st.cull = false,
            gl::SCISSOR_TEST => self.st.scissor = None,
            _ => {}
        }
    }
    pub fn blend_func_separate(&mut self, src_rgb: u32, dst_rgb: u32, src_a: u32, dst_a: u32) {
        self.st.blend_src_rgb = src_rgb;
        self.st.blend_dst_rgb = dst_rgb;
        self.st.blend_src_a = src_a;
        self.st.blend_dst_a = dst_a;
    }
    pub fn depth_func(&mut self, f: u32) {
        self.st.depth_func = f;
    }
    pub fn depth_mask(&mut self, on: bool) {
        self.st.depth_mask = on;
    }
    pub fn front_face(&mut self, f: u32) {
        self.st.front_face = f;
    }
    pub fn cull_face(&mut self, f: u32) {
        self.st.cull_face = f;
    }
    pub fn viewport(&mut self, x: i32, y: i32, w: i32, h: i32) {
        self.st.viewport = [x, y, w, h];
    }
    pub fn scissor(&mut self, x: i32, y: i32, w: i32, h: i32) {
        self.st.scissor = Some([x, y, w, h]);
    }
    pub fn clear_color(&mut self, r: f64, g: f64, b: f64, a: f64) {
        self.st.clear_color = [r, g, b, a];
    }
    pub fn clear_depth(&mut self, d: f64) {
        self.st.clear_depth = d.clamp(0.0, 1.0);
    }
    pub fn use_program(&mut self, id: u32) {
        self.st.program = id;
    }
    pub fn clear(&mut self, mask: u32) {
        if mask & gl::COLOR_BUFFER_BIT != 0 || mask & gl::DEPTH_BUFFER_BIT != 0 {
            self.st.pending_clear = true;
        }
    }
    pub fn get_error(&mut self) -> u32 {
        let e = self.st.last_error;
        self.st.last_error = gl::NO_ERROR;
        e
    }
    /// UNPACK_FLIP_Y_WEBGL state (for the texture-upload path).
    pub fn flip_y(&self) -> bool {
        self.st.flip_y
    }

    // -- draw -------------------------------------------------------------------

    fn blend_factor(f: u32) -> Option<wgpu::BlendFactor> {
        use wgpu::BlendFactor as F;
        Some(match f {
            gl::ZERO => F::Zero,
            gl::ONE => F::One,
            gl::SRC_COLOR => F::Src,
            gl::ONE_MINUS_SRC_COLOR => F::OneMinusSrc,
            gl::SRC_ALPHA => F::SrcAlpha,
            gl::ONE_MINUS_SRC_ALPHA => F::OneMinusSrcAlpha,
            gl::DST_ALPHA => F::DstAlpha,
            gl::ONE_MINUS_DST_ALPHA => F::OneMinusDstAlpha,
            gl::DST_COLOR => F::Dst,
            gl::ONE_MINUS_DST_COLOR => F::OneMinusDst,
            _ => return None,
        })
    }

    fn depth_compare(f: u32) -> Option<wgpu::CompareFunction> {
        use wgpu::CompareFunction as C;
        Some(match f {
            gl::NEVER => C::Never,
            gl::LESS => C::Less,
            gl::EQUAL => C::Equal,
            gl::LEQUAL => C::LessEqual,
            gl::GREATER => C::Greater,
            gl::NOTEQUAL => C::NotEqual,
            gl::GEQUAL => C::GreaterEqual,
            gl::ALWAYS => C::Always,
            _ => return None,
        })
    }

    fn topology(mode: u32) -> Option<wgpu::PrimitiveTopology> {
        use wgpu::PrimitiveTopology as T;
        match mode {
            gl::POINTS => Some(T::PointList),
            gl::LINES => Some(T::LineList),
            gl::LINE_STRIP => Some(T::LineStrip),
            gl::TRIANGLES => Some(T::TriangleList),
            gl::TRIANGLE_STRIP => Some(T::TriangleStrip),
            _ => None, // TRIANGLE_FAN / LINE_LOOP unsupported (documented)
        }
    }

    /// Execute one draw: build/lookup the pipeline, gather resources, run one
    /// render pass over the canvas color target.
    pub fn draw(&mut self, mode: u32, first: i32, count: i32, indexed: bool, index_type: u32) {
        let Some(topo) = Self::topology(mode) else {
            err(&mut self.st, gl::INVALID_ENUM);
            return;
        };
        if count <= 0 {
            return;
        }

        // ---- phase 1: gather all state (owned clones, no &self borrows) ----
        let program_id = self.st.program;
        let (vs_wgsl, fs_wgsl, vs_entry, fs_entry, uniforms_meta, tex_meta, uniform_shadows) = {
            let Some(program) = self.objects.programs.get(&program_id) else {
                err(&mut self.st, gl::INVALID_OPERATION);
                return;
            };
            let Some(linked) = program.linked.as_ref() else {
                err(&mut self.st, gl::INVALID_OPERATION);
                return;
            };
            let shadows: Vec<(u32, Vec<u8>)> =
                linked.uniform_shadows.iter().map(|(b, v)| (*b, v.clone())).collect();
            let uniforms: Vec<u32> = linked.uniform_kinds.keys().copied().collect();
            let textures: Vec<(u32, ())> = linked
                .vs
                .textures
                .keys()
                .chain(linked.fs.textures.keys())
                .map(|b| (*b, ()))
                .collect();
            (
                linked.vs.wgsl.clone(),
                linked.fs.wgsl.clone(),
                linked.vs.entry.clone(),
                linked.fs.entry.clone(),
                uniforms,
                textures,
                shadows,
            )
        };

        let attribs: Vec<(u32, GlAttrib)> = {
            let list: Vec<(u32, GlAttrib)> = if self.st.vao != 0 {
                self.objects
                    .vaos
                    .get(&self.st.vao)
                    .map(|v| v.attribs.iter().map(|(k, a)| (*k, a.clone())).collect())
                    .unwrap_or_default()
            } else {
                self.st.default_vao.attribs.iter().map(|(k, a)| (*k, a.clone())).collect()
            };
            list.into_iter()
                .filter(|(idx, _)| *idx < 16 && self.st.attrib_enabled[*idx as usize])
                .collect()
        };
        // Per-attrib vertex data snapshots.
        let mut attrib_data: Vec<(u32, GlAttrib, Vec<u8>)> = Vec::new();
        for (idx, a) in &attribs {
            match self.objects.buffers.get(&a.buffer) {
                Some(b) => attrib_data.push((*idx, a.clone(), b.data.clone())),
                None => {
                    err(&mut self.st, gl::INVALID_OPERATION);
                    return;
                }
            }
        }
        // Index buffer snapshot.
        let index_data: Option<Vec<u8>> = if indexed {
            let ebuf = if self.st.vao != 0 {
                self.objects.vaos.get(&self.st.vao).map(|v| v.element_buffer).unwrap_or(0)
            } else {
                self.st.element_buffer
            };
            match self.objects.buffers.get(&ebuf) {
                Some(b) => Some(b.data.clone()),
                None => {
                    err(&mut self.st, gl::INVALID_OPERATION);
                    return;
                }
            }
        } else {
            None
        };
        // Texture snapshots (binding -> (w, h, pixels, filters)).
        struct TexSnap {
            binding: u32,
            w: u32,
            h: u32,
            pixels: Vec<u8>,
            min_f: u32,
            mag_f: u32,
            wrap_s: u32,
            wrap_t: u32,
        }
        let mut tex_snaps: Vec<TexSnap> = Vec::new();
        for (b, _) in &tex_meta {
            // Binding 100.. map to units 0.. (TEXTURE0 + unit).
            let unit = (*b as usize).saturating_sub(100).min(15);
            let tex_id = self.st.textures[unit];
            if let Some(t) = self.objects.textures.get(&tex_id) {
                tex_snaps.push(TexSnap {
                    binding: *b,
                    w: t.width.max(1),
                    h: t.height.max(1),
                    pixels: t.pixels.clone(),
                    min_f: t.min_filter,
                    mag_f: t.mag_filter,
                    wrap_s: t.wrap_s,
                    wrap_t: t.wrap_t,
                });
            }
        }
        let st = self.st.clone();
        let gpu = self.gpu.clone();

        // ---- phase 2: device resources --------------------------------------
        // Vertex buffers (rebuilt each draw from snapshots; small data).
        let mut vbufs: Vec<(wgpu::Buffer, u64, wgpu::VertexFormat, u64, u32)> = Vec::new();
        for (idx, a, data) in &attrib_data {
            let type_size = if a.typ == gl::FLOAT { 4usize } else { 1 };
            let packed = (a.size as usize) * type_size;
            let stride = if a.stride == 0 { packed } else { a.stride as usize };
            if stride % 4 != 0 || (a.offset as usize) % 4 != 0 {
                // wgpu requires 4-byte alignment for vertex strides/offsets.
                err(&mut self.st, gl::INVALID_VALUE);
                return;
            }
            let format = match (a.typ, a.size, a.normalized) {
                (gl::FLOAT, 1, _) => wgpu::VertexFormat::Float32,
                (gl::FLOAT, 2, _) => wgpu::VertexFormat::Float32x2,
                (gl::FLOAT, 3, _) => wgpu::VertexFormat::Float32x3,
                (gl::FLOAT, 4, _) => wgpu::VertexFormat::Float32x4,
                (gl::UNSIGNED_BYTE, 4, true) => wgpu::VertexFormat::Unorm8x4,
                _ => {
                    err(&mut self.st, gl::INVALID_ENUM);
                    return;
                }
            };
            let buf = gpu.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("brows12-webgl-vbo"),
                size: data.len() as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: true,
            });
            buf.slice(..).get_mapped_range_mut().expect("map vbuf").copy_from_slice(data);
            buf.unmap();
            vbufs.push((buf, stride as u64, format, a.offset as u64, *idx));
        }
        let index_buf = index_data.map(|data| {
            let buf = gpu.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("brows12-webgl-ibo"),
                size: data.len() as u64,
                usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: true,
            });
            buf.slice(..).get_mapped_range_mut().expect("map ibo").copy_from_slice(&data);
            buf.unmap();
            buf
        });

        // Pipeline (cache key from state + layout).
        let mut key = std::collections::hash_map::DefaultHasher::new();
        use std::hash::Hasher as _;
        key.write_u32(program_id);
        key.write_u8(st.blend as u8);
        key.write_u32(st.blend_src_rgb);
        key.write_u32(st.blend_dst_rgb);
        key.write_u32(st.blend_src_a);
        key.write_u32(st.blend_dst_a);
        key.write_u8(st.depth_test as u8);
        key.write_u32(st.depth_func);
        key.write_u8(st.depth_mask as u8);
        key.write_u8(st.cull as u8);
        key.write_u32(st.cull_face);
        key.write_u32(st.front_face);
        key.write_u32(mode);
        for (idx, a, _) in &attrib_data {
            key.write_u32(*idx);
            key.write_u32(a.size as u32);
            key.write_u32(a.typ);
            key.write_u8(a.normalized as u8);
            key.write_u32(a.stride as u32);
            key.write_u32(a.offset);
        }
        let key = key.finish();

        let bgl0_entries: Vec<wgpu::BindGroupLayoutEntry> = uniforms_meta
            .iter()
            .map(|b| wgpu::BindGroupLayoutEntry {
                binding: *b,
                visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            })
            .chain(tex_snaps.iter().map(|t| wgpu::BindGroupLayoutEntry {
                binding: t.binding,
                visibility: wgpu::ShaderStages::FRAGMENT | wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            }))
            .collect();
        let bgl1_entries: Vec<wgpu::BindGroupLayoutEntry> = tex_snaps
            .iter()
            .map(|t| wgpu::BindGroupLayoutEntry {
                binding: t.binding,
                visibility: wgpu::ShaderStages::FRAGMENT | wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            })
            .collect();

        let pipeline = if let Some(p) = self.pipelines.get(&key) {
            p.clone()
        } else {
            let bgl0 = gpu.device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("brows12-webgl-bgl0"),
                entries: &bgl0_entries,
            });
            let bgl1 = gpu.device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("brows12-webgl-bgl1"),
                entries: &bgl1_entries,
            });
            let layout = gpu.device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("brows12-webgl-pl"),
                bind_group_layouts: &[Some(&bgl0), Some(&bgl1)],
                immediate_size: 0,
            });
            let vs_mod = gpu.device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("brows12-webgl-vs"),
                source: wgpu::ShaderSource::Wgsl(vs_wgsl.into()),
            });
            let fs_mod = gpu.device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("brows12-webgl-fs"),
                source: wgpu::ShaderSource::Wgsl(fs_wgsl.into()),
            });
            let depth = if st.depth_test {
                Some(wgpu::DepthStencilState {
                    format: wgpu::TextureFormat::Depth24Plus,
                    depth_write_enabled: Some(st.depth_mask),
                    depth_compare: Some(
                        Self::depth_compare(st.depth_func).unwrap_or(wgpu::CompareFunction::Less),
                    ),
                    stencil: wgpu::StencilState::default(),
                    bias: wgpu::DepthBiasState::default(),
                })
            } else {
                None
            };
            let blend = if st.blend {
                let f = |src: u32, dst: u32| wgpu::BlendComponent {
                    src_factor: Self::blend_factor(src).unwrap_or(wgpu::BlendFactor::One),
                    dst_factor: Self::blend_factor(dst).unwrap_or(wgpu::BlendFactor::Zero),
                    operation: wgpu::BlendOperation::Add,
                };
                Some(wgpu::BlendState {
                    color: f(st.blend_src_rgb, st.blend_dst_rgb),
                    alpha: f(st.blend_src_a, st.blend_dst_a),
                })
            } else {
                None
            };
            let cull = if st.cull {
                Some(match st.cull_face {
                    gl::FRONT => wgpu::Face::Front,
                    _ => wgpu::Face::Back,
                })
            } else {
                None
            };
            let attr_arrays: Vec<[wgpu::VertexAttribute; 1]> = vbufs
                .iter()
                .map(|(_, _, fmt, off, loc)| {
                    [wgpu::VertexAttribute { format: *fmt, offset: *off, shader_location: *loc }]
                })
                .collect();
            let layouts: Vec<Option<wgpu::VertexBufferLayout>> = vbufs
                .iter()
                .zip(attr_arrays.iter())
                .map(|((_, stride, _, _, _), arr)| {
                    Some(wgpu::VertexBufferLayout {
                        array_stride: *stride,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: arr.as_slice(),
                    })
                })
                .collect();
            let pipeline = gpu.device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("brows12-webgl-pipeline"),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &vs_mod,
                    entry_point: Some(&vs_entry),
                    buffers: &layouts,
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &fs_mod,
                    entry_point: Some(&fs_entry),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: wgpu::TextureFormat::Rgba8Unorm,
                        blend,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: Default::default(),
                }),
                primitive: wgpu::PrimitiveState {
                    topology: topo,
                    cull_mode: cull,
                    front_face: if st.front_face == gl::CW {
                        wgpu::FrontFace::Cw
                    } else {
                        wgpu::FrontFace::Ccw
                    },
                    ..Default::default()
                },
                depth_stencil: depth,
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            });
            self.pipelines.insert(key, pipeline.clone());
            pipeline
        };

        // Bind group 0: uniform buffers + texture views.
        let mut ubufs: Vec<wgpu::Buffer> = Vec::new();
        let mut bg0_entries: Vec<wgpu::BindGroupEntry> = Vec::new();
        for (b, shadow) in &uniform_shadows {
            let need = (shadow.len().max(4)) as u64;
            let dev = match self.uniform_bufs.get(&(program_id, *b)) {
                Some((buf, cap)) if *cap >= need => buf.clone(),
                _ => {
                    let buf = gpu.device.create_buffer(&wgpu::BufferDescriptor {
                        label: Some("brows12-webgl-ubo"),
                        size: need,
                        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                        mapped_at_creation: false,
                    });
                    self.uniform_bufs.insert((program_id, *b), (buf.clone(), need));
                    buf
                }
            };
            if !shadow.is_empty() {
                gpu.queue.write_buffer(&dev, 0, shadow);
            }
            ubufs.push(dev);
        }
        for (i, (b, _)) in uniform_shadows.iter().enumerate() {
            bg0_entries
                .push(wgpu::BindGroupEntry { binding: *b, resource: ubufs[i].as_entire_binding() });
        }
        struct TexHandles {
            binding: u32,
            view: wgpu::TextureView,
            sampler: wgpu::Sampler,
        }
        let mut tex_handles: Vec<TexHandles> = Vec::new();
        for t in &tex_snaps {
            let tex = gpu.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("brows12-webgl-tex"),
                size: wgpu::Extent3d { width: t.w, height: t.h, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            if !t.pixels.is_empty() {
                gpu.queue.write_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture: &tex,
                        mip_level: 0,
                        origin: wgpu::Origin3d::ZERO,
                        aspect: wgpu::TextureAspect::All,
                    },
                    &t.pixels,
                    wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(t.w * 4),
                        rows_per_image: Some(t.h),
                    },
                    wgpu::Extent3d { width: t.w, height: t.h, depth_or_array_layers: 1 },
                );
            }
            let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
            let filt = |f: u32| {
                if f == gl::LINEAR {
                    wgpu::FilterMode::Linear
                } else {
                    wgpu::FilterMode::Nearest
                }
            };
            let addr = |f: u32| {
                if f == gl::CLAMP_TO_EDGE {
                    wgpu::AddressMode::ClampToEdge
                } else {
                    wgpu::AddressMode::Repeat
                }
            };
            let sampler = gpu.device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("brows12-webgl-sampler"),
                mag_filter: filt(t.mag_f),
                min_filter: filt(t.min_f),
                address_mode_u: addr(t.wrap_s),
                address_mode_v: addr(t.wrap_t),
                ..Default::default()
            });
            tex_handles.push(TexHandles { binding: t.binding, view, sampler });
        }
        for t in &tex_handles {
            bg0_entries.push(wgpu::BindGroupEntry {
                binding: t.binding,
                resource: wgpu::BindingResource::TextureView(&t.view),
            });
        }

        // Bind group 1: samplers (set 1 per the normalized shader source).
        let mut bg1_entries: Vec<wgpu::BindGroupEntry> = Vec::new();
        for t in &tex_handles {
            bg1_entries.push(wgpu::BindGroupEntry {
                binding: t.binding,
                resource: wgpu::BindingResource::Sampler(&t.sampler),
            });
        }

        let bgl0 = pipeline.get_bind_group_layout(0);
        let bgl1 = pipeline.get_bind_group_layout(1);
        let bg0 = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("brows12-webgl-bg0"),
            layout: &bgl0,
            entries: &bg0_entries,
        });
        let bg1 = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("brows12-webgl-bg1"),
            layout: &bgl1,
            entries: &bg1_entries,
        });

        // ---- phase 3: encode -------------------------------------------------
        let (vx, vy, vw, vh) = (
            st.viewport[0].max(0),
            st.viewport[1].max(0),
            st.viewport[2].max(1),
            st.viewport[3].max(1),
        );
        let mut encoder = gpu.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("brows12-webgl-enc"),
        });
        let depth_view = self.depth.as_ref().map(|d| d.create_view(&Default::default()));
        let clear_color = if st.pending_clear {
            wgpu::LoadOp::Clear(wgpu::Color {
                r: st.clear_color[0],
                g: st.clear_color[1],
                b: st.clear_color[2],
                a: st.clear_color[3],
            })
        } else {
            wgpu::LoadOp::Load
        };
        let clear_depth = st.clear_depth;
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("brows12-webgl-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.color.create_view(&Default::default()),
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations { load: clear_color, store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: depth_view.as_ref().map(|v| {
                    wgpu::RenderPassDepthStencilAttachment {
                        view: v,
                        depth_ops: Some(wgpu::Operations {
                            load: if st.pending_clear {
                                wgpu::LoadOp::Clear(clear_depth as f32)
                            } else {
                                wgpu::LoadOp::Load
                            },
                            store: wgpu::StoreOp::Store,
                        }),
                        stencil_ops: None,
                    }
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            let (sw, sh, sx, sy) = match st.scissor {
                Some([x, y, w, h]) => {
                    (w.max(0) as u32, h.max(0) as u32, x.max(0) as u32, y.max(0) as u32)
                }
                None => (self.width, self.height, 0, 0),
            };
            pass.set_scissor_rect(sx, sy, sw.max(1), sh.max(1));
            let vf = vw as f32;
            let _ = vf;
            pass.set_viewport(vx as f32, vy as f32, vw as f32, vh as f32, 0.0, 1.0);
            pass.set_pipeline(&pipeline);
            // Always bind both groups (empty layouts are valid; the pipeline
            // expects a bind group at every index it declares).
            pass.set_bind_group(0, &bg0, &[]);
            pass.set_bind_group(1, &bg1, &[]);
            for (buf, _, _, _, idx) in vbufs.iter() {
                pass.set_vertex_buffer(*idx, buf.slice(..));
            }
            if let (true, Some(ibuf)) = (indexed, &index_buf) {
                let fmt = if index_type == gl::UNSIGNED_INT {
                    wgpu::IndexFormat::Uint32
                } else {
                    wgpu::IndexFormat::Uint16
                };
                pass.set_index_buffer(ibuf.slice(..), fmt);
                pass.draw_indexed(first as u32..(first + count) as u32, 0, 0..1);
            } else {
                pass.draw(first as u32..(first + count) as u32, 0..1);
            }
        }
        self.st.pending_clear = false;
        gpu.queue.submit([encoder.finish()]);
    }

    /// Read back the color target: (width, height, straight-alpha RGBA).
    /// Applies a pending clear first. Returns None when no GPU is present.
    pub fn readback(&mut self) -> Option<Vec<u8>> {
        let gpu = self.gpu.clone();
        if self.st.pending_clear {
            // Flush the clear through an empty pass.
            let view = self.color.create_view(&Default::default());
            let mut enc =
                gpu.device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
            let c = self.st.clear_color;
            enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("brows12-webgl-clear"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: c[0],
                            g: c[1],
                            b: c[2],
                            a: c[3],
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            self.st.pending_clear = false;
            gpu.queue.submit([enc.finish()]);
        }
        let w = self.width;
        let h = self.height;
        let bytes_per_row = (w * 4).div_ceil(256) * 256;
        let stage = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("brows12-webgl-readback"),
            size: (bytes_per_row * h) as u64,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut enc = gpu.device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        enc.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &self.color,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &stage,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(bytes_per_row),
                    rows_per_image: Some(h),
                },
            },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        gpu.queue.submit([enc.finish()]);
        let slice = stage.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        let _ = gpu.device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None });
        match rx.recv() {
            Ok(Ok(())) => {}
            _ => return None,
        }
        let mapped = slice.get_mapped_range().ok()?;
        let mut premul = Vec::with_capacity((w * h * 4) as usize);
        for row in 0..h {
            let start = (row * bytes_per_row) as usize;
            premul.extend_from_slice(&mapped[start..start + (w * 4) as usize]);
        }
        drop(mapped);
        stage.unmap();
        // WebGL default: premultipliedAlpha = true → convert for the image
        // pipeline (straight alpha).
        Some(crate::canvas::canvas_unpremultiply(&premul))
    }

    /// getAttribLocation: the WebGL attribute index (= our injected location).
    pub fn get_attrib_location(&self, prog: u32, name: &str) -> Option<i32> {
        self.objects
            .programs
            .get(&prog)?
            .linked
            .as_ref()?
            .attributes
            .get(name)
            .copied()
            .map(|l| l as i32)
    }
}

/// std140-ish byte size for the uniform types real shaders use
/// (scalars, vectors, matrices). Documented subset.
fn std140_size(inner: &naga::TypeInner) -> u32 {
    match inner {
        naga::TypeInner::Scalar(_) => 4,
        naga::TypeInner::Vector { size, .. } => (*size as u32) * 4,
        naga::TypeInner::Matrix { columns, .. } => (*columns as u32) * 16, // col stride 16
        _ => 64, // conservative fallback for documented-unsupported types
    }
}
