//! v2.1 perf: GPU-side present path (the fast lane).
//!
//! The old present path was pure CPU and ran *per frame*:
//!
//! 1. `webview.paint()` renders the page into the offscreen FBO (GPU — good);
//! 2. `read_to_image()` — `glReadPixels` of the whole 1280×728×4 framebuffer.
//!    This is the killer: it stalls the entire GPU pipeline and memcpy's
//!    ~3.7 MB to the CPU *every frame*;
//! 3. a per-pixel premultiply copy into a tiny-skia pixmap;
//! 4. a full-window tiny-skia composite (chrome + page);
//! 5. a per-pixel demultiply + `0RGB` conversion into the softbuffer;
//! 6. softbuffer's own blit to the window.
//!
//! Steps 2–6 are 5 full-window CPU passes + one GPU pipeline flush per
//! frame. On an integrated-GPU laptop that is the difference between a
//! 60 fps compositor and a ~8–15 fps slideshow (measured, see
//! docs/V2_1_PERF_REPORT.md).
//!
//! The fast path keeps everything on the GPU:
//!
//! 1. `webview.paint()` renders into the offscreen FBO (unchanged);
//! 2. `OffscreenRenderingContext::render_to_parent_callback()` — a GPU
//!    `glBlitFramebuffer` from the webview FBO straight into the window
//!    surface (servo 0.6's designed integration path; zero CPU pixels);
//! 3. the chrome strip (tab bar + toolbar) is drawn once per frame into a
//!    small 1280×72 CPU pixmap (unchanged — it is 10× smaller than the
//!    viewport), uploaded as a texture only when it changed, and blended
//!    on top with a textured quad;
//! 4. `parent.present()` — the surfman swap chain flip.
//!
//! Per frame CPU cost drops from ~5 full-window passes + readback to one
//! 1280×72 pixmap draw + (only when chrome changed) a 360 KB texture
//! upload.

use glow::HasContext;
use std::rc::Rc;
use std::sync::Arc;

use servo::RenderingContext;

pub const CHROME_QUAD_VERT_SRC: &str = r#"#version 100
attribute vec2 a_pos;
attribute vec2 a_uv;
varying vec2 v_uv;
void main() {
    // GL clip space: -1..1, y up. The strip texture is uploaded top-row-first,
    // so v=0 is the top of the strip and v=1 the bottom.
    gl_Position = vec4(a_pos, 0.0, 1.0);
    v_uv = a_uv;
}
"#;

pub const CHROME_QUAD_FRAG_SRC: &str = r#"#version 100
precision mediump float;
uniform sampler2D u_tex;
varying vec2 v_uv;
void main() {
    vec4 c = texture2D(u_tex, v_uv);
    // Premultiplied alpha source (tiny-skia output), straight-alpha output.
    gl_FragColor = vec4(c.rgb / max(c.a, 0.0001), c.a);
}
"#;

/// A textured-quad renderer + the chrome strip texture, bound to the
/// parent window rendering context. Created lazily on the first GPU draw.
pub struct ChromeOverlay {
    gl: Arc<glow::Context>,
    program: glow::Program,
    vao: Option<glow::VertexArray>,
    vbo: glow::Buffer,
    ebo: glow::Buffer,
    tex: glow::Texture,
    loc_pos: glow::UniformLocation,
    loc_uv: glow::UniformLocation,
    loc_tex: glow::UniformLocation,
    /// Texture dimensions currently allocated (for resize).
    tex_w: u32,
    tex_h: u32,
    /// The last composited chrome strip (BGRA, premultiplied) + its params,
    /// so we can re-upload after a resize without re-rasterizing.
    last_w: u32,
    last_h: u32,
    last_pixels: Vec<u8>,
}

impl ChromeOverlay {
    pub fn new(parent: &Rc<dyn RenderingContext>) -> Result<Self, String> {
        let gl = parent.glow_gl_api();
        unsafe {
            let program = gl
                .create_program()
                .map_err(|e| format!("create_program: {e:?}"))?;
            let vs = gl
                .create_shader(glow::VERTEX_SHADER)
                .map_err(|e| format!("create_shader: {e:?}"))?;
            gl.shader_source(vs, CHROME_QUAD_VERT_SRC);
            gl.compile_shader(vs);
            let vs_ok = gl.get_shader_compile_status(vs);
            let vs_log = gl.get_shader_info_log(vs);
            gl.attach_shader(program, vs);
            gl.delete_shader(vs);
            if !vs_ok {
                return Err(format!("chrome vert shader: {vs_log}"));
            }
            let fs = gl
                .create_shader(glow::FRAGMENT_SHADER)
                .map_err(|e| format!("create_shader: {e:?}"))?;
            gl.shader_source(fs, CHROME_QUAD_FRAG_SRC);
            gl.compile_shader(fs);
            let fs_ok = gl.get_shader_compile_status(fs);
            let fs_log = gl.get_shader_info_log(fs);
            gl.attach_shader(program, fs);
            gl.delete_shader(fs);
            if !fs_ok {
                return Err(format!("chrome frag shader: {fs_log}"));
            }
            gl.link_program(program);
            let ok = gl.get_program_link_status(program);
            let log = gl.get_program_info_log(program);
            if !ok {
                gl.delete_program(program);
                return Err(format!("chrome program link: {log}"));
            }
            // Core-profile contexts (Mesa GL 4.5 core on Linux, GLES3 on
            // ANGLE) reject draws without a real VAO — create one and keep
            // the quad's vertex setup persistent in it.
            let vao = gl.create_vertex_array().ok();
            gl.bind_vertex_array(vao);
            let vbo = gl.create_buffer().map_err(|e| format!("vbo: {e:?}"))?;
            let ebo = gl.create_buffer().map_err(|e| format!("ebo: {e:?}"))?;
            let tex = gl.create_texture().map_err(|e| format!("tex: {e:?}"))?;
            gl.bind_texture(glow::TEXTURE_2D, Some(tex));
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_MIN_FILTER,
                glow::NEAREST as i32,
            );
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_MAG_FILTER,
                glow::NEAREST as i32,
            );
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_WRAP_S,
                glow::CLAMP_TO_EDGE as i32,
            );
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_WRAP_T,
                glow::CLAMP_TO_EDGE as i32,
            );
            gl.bind_texture(glow::TEXTURE_2D, None);
            let loc_pos = glow::NativeUniformLocation(
                gl.get_attrib_location(program, "a_pos")
                    .ok_or("no a_pos loc")?,
            );
            let loc_uv = glow::NativeUniformLocation(
                gl.get_attrib_location(program, "a_uv").ok_or("no a_uv loc")?,
            );
            let loc_tex = gl
                .get_uniform_location(program, "u_tex")
                .ok_or("no u_tex loc")?;
            gl.bind_vertex_array(None);
            Ok(Self {
                gl,
                program,
                vao,
                vbo,
                ebo,
                tex,
                loc_pos,
                loc_uv,
                loc_tex,
                tex_w: 0,
                tex_h: 0,
                last_w: 0,
                last_h: 0,
                last_pixels: Vec::new(),
            })
        }
    }

    /// Upload (or re-upload) the chrome strip. `bgra` is premultiplied
    /// tiny-skia output, rows top-to-bottom.
    pub fn upload_chrome(&mut self, w: u32, h: u32, bgra: &[u8]) {
        unsafe {
            let gl = &self.gl;
            gl.bind_texture(glow::TEXTURE_2D, Some(self.tex));
            if self.tex_w != w || self.tex_h != h {
                gl.tex_image_2d(
                    glow::TEXTURE_2D,
                    0,
                    glow::RGBA as i32,
                    w as i32,
                    h as i32,
                    0,
                    glow::RGBA,
                    glow::UNSIGNED_BYTE,
                    glow::PixelUnpackData::Slice(None),
                );
                self.tex_w = w;
                self.tex_h = h;
            }
            // tiny-skia stores BGRA; GL wants RGBA byte order.
            let mut rgba = Vec::with_capacity(bgra.len());
            for px in bgra.chunks_exact(4) {
                rgba.push(px[2]);
                rgba.push(px[1]);
                rgba.push(px[0]);
                rgba.push(px[3]);
            }
            gl.tex_sub_image_2d(
                glow::TEXTURE_2D,
                0,
                0,
                0,
                w as i32,
                h as i32,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelUnpackData::Slice(Some(&rgba)),
            );
            gl.bind_texture(glow::TEXTURE_2D, None);
        }
        self.last_w = w;
        self.last_h = h;
        self.last_pixels = bgra.to_vec();
    }

    /// Re-upload after the overlay was recreated (e.g. resize).
    pub fn restore_last(&mut self) {
        if self.last_w > 0 && !self.last_pixels.is_empty() {
            let (w, h) = (self.last_w, self.last_h);
            let px = std::mem::take(&mut self.last_pixels);
            self.upload_chrome(w, h, &px);
        }
    }

    /// Draw the chrome strip blended over the *currently bound* framebuffer.
    /// `win_w`/`win_h` are the framebuffer size in pixels; the strip covers
    /// the top `chrome_h` rows (top-down screen coordinates).
    pub fn draw(&self, win_w: i32, win_h: i32, chrome_h: i32) {
        unsafe {
            let gl = &self.gl;
            gl.bind_vertex_array(self.vao);
            gl.use_program(Some(self.program));
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, Some(self.tex));
            gl.uniform_1_i32(Some(&self.loc_tex), 0);

            // Quad covering the top strip: clip-space y from
            // 1.0 - 2*chrome_h/win_h (top edge) to 1.0 (bottom edge of strip).
            let y_top = 1.0;
            let y_bottom = 1.0 - 2.0 * chrome_h as f32 / win_h.max(1) as f32;
            // uv: v=0 top row of the texture, v=1 bottom row.
            let verts: [f32; 16] = [
                // pos (clip)        uv
                -1.0, y_top, 0.0, 0.0, // top-left
                1.0, y_top, 1.0, 0.0, // top-right
                1.0, y_bottom, 1.0, 1.0, // bottom-right
                -1.0, y_bottom, 0.0, 1.0, // bottom-left
            ];
            let indices: [u16; 6] = [0, 1, 2, 0, 2, 3];

            gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.vbo));
            gl.buffer_data_u8_slice(
                glow::ARRAY_BUFFER,
                bytemuck::cast_slice(&verts),
                glow::DYNAMIC_DRAW,
            );
            gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(self.ebo));
            gl.buffer_data_u8_slice(
                glow::ELEMENT_ARRAY_BUFFER,
                bytemuck::cast_slice(&indices),
                glow::DYNAMIC_DRAW,
            );

            let stride = 4 * std::mem::size_of::<f32>() as i32;
            gl.enable_vertex_attrib_array(self.loc_pos.0);
            gl.vertex_attrib_pointer_f32(self.loc_pos.0, 2, glow::FLOAT, false, stride, 0);
            gl.enable_vertex_attrib_array(self.loc_uv.0);
            gl.vertex_attrib_pointer_f32(
                self.loc_uv.0,
                2,
                glow::FLOAT,
                false,
                stride,
                2 * std::mem::size_of::<f32>() as i32,
            );

            gl.viewport(0, 0, win_w, win_h);
            gl.disable(glow::SCISSOR_TEST);
            gl.enable(glow::BLEND);
            gl.blend_func_separate(
                glow::ONE,
                glow::ONE_MINUS_SRC_ALPHA,
                glow::ONE,
                glow::ONE_MINUS_SRC_ALPHA,
            );
            gl.draw_elements(glow::TRIANGLES, 6, glow::UNSIGNED_SHORT, 0);
            gl.disable(glow::BLEND);
            gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, None);
            gl.bind_buffer(glow::ARRAY_BUFFER, None);
            gl.bind_texture(glow::TEXTURE_2D, None);
            gl.use_program(None);
        }
    }
}

impl Drop for ChromeOverlay {
    fn drop(&mut self) {
        unsafe {
            let gl = &self.gl;
            gl.delete_program(self.program);
            gl.delete_buffer(self.vbo);
            gl.delete_buffer(self.ebo);
            gl.delete_texture(self.tex);
            if let Some(vao) = self.vao {
                gl.delete_vertex_array(vao);
            }
        }
    }
}

// tiny-skia Pixmap pixel access is BGRA premultiplied — we depend on that
// byte order above. `bytemuck` is available transitively; keep the cast
// dependency-free with a manual helper instead.
mod bytemuck {
    pub fn cast_slice<T>(src: &[T]) -> &[u8] {
        let len = std::mem::size_of_val(src);
        unsafe { std::slice::from_raw_parts(src.as_ptr() as *const u8, len) }
    }
}
