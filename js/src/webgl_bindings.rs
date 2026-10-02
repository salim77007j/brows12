//! WebGL 2 natives: `__brows12.gl*` functions called from glue.js.
//!
//! Every GL call takes the canvas node id first; the surface is looked up in
//! the realm's [`WebGlStore`]. When no GPU adapter exists the store is empty
//! and getContext already returned null, so these are never reached.

use crate::environment::JsEnvironment;
use crate::webgl::gl;
use rquickjs::{Ctx, Function, Object, Value};
use std::sync::Arc;

/// Extract f32 payload from a JS value: Float32Array/Int32Array/Uint8Array
/// or a plain array of numbers.
fn f32s<'js>(v: &Value<'js>) -> Option<Vec<f32>> {
    if let Some(arr) = crate::glue::as_typed_f32(v) {
        return Some(arr);
    }
    let arr = v.as_array()?;
    let mut out = Vec::new();
    for item in arr.iter::<f64>() {
        out.push(item.ok()? as f32);
    }
    Some(out)
}

fn bytes<'js>(v: &Value<'js>) -> Option<Vec<u8>> {
    if let Some(b) = crate::glue::as_typed_u8(v) {
        return Some(b);
    }
    let arr = v.as_array()?;
    let mut out = Vec::new();
    for item in arr.iter::<f64>() {
        out.push(item.ok()? as u8);
    }
    Some(out)
}

macro_rules! surface {
    ($store:expr, $id:expr) => {
        match $store.surface($id) {
            Some(s) => s,
            None => return Default::default(),
        }
    };
}

/// Install the `__brows12` WebGL natives.
pub fn install<'js>(
    ctx: &Ctx<'js>,
    env: &Arc<JsEnvironment>,
    ns: &Object<'js>,
) -> rquickjs::Result<()> {
    let store = env.webgl_store.clone();
    let canvas2d_store = env.canvas_store.clone();

    // ---- context creation ---------------------------------------------------
    ns.set(
        "glGetContext",
        Function::new(ctx.clone(), {
            let store = store.clone();
            move |id: i32, w: f64, h: f64| -> Option<Vec<String>> {
                let (surface, name, backend) =
                    store.get_context(id, w.max(1.0) as u32, h.max(1.0) as u32)?;
                drop(surface);
                Some(vec![name, backend])
            }
        })?,
    )?;

    // ---- object creation ------------------------------------------------------
    ns.set(
        "glCreateBuffer",
        Function::new(ctx.clone(), {
            let store = store.clone();
            move |id: i32| -> u32 {
                let s = surface!(store, id);
                let mut s = s.lock().unwrap();
                s.create_buffer()
            }
        })?,
    )?;
    ns.set(
        "glDeleteBuffer",
        Function::new(ctx.clone(), {
            let store = store.clone();
            move |id: i32, b: u32| {
                let s = surface!(store, id);
                let mut s = s.lock().unwrap();
                s.delete_buffer(b);
            }
        })?,
    )?;
    ns.set(
        "glCreateShader",
        Function::new(ctx.clone(), {
            let store = store.clone();
            move |id: i32, stage: u32| -> u32 {
                let s = surface!(store, id);
                let mut s = s.lock().unwrap();
                s.create_shader(stage)
            }
        })?,
    )?;
    ns.set(
        "glShaderSource",
        Function::new(ctx.clone(), {
            let store = store.clone();
            move |id: i32, sh: u32, src: String| {
                let s = surface!(store, id);
                let mut s = s.lock().unwrap();
                s.shader_source(sh, src);
            }
        })?,
    )?;
    ns.set(
        "glCompileShader",
        Function::new(ctx.clone(), {
            let store = store.clone();
            move |id: i32, sh: u32| {
                let s = surface!(store, id);
                let mut s = s.lock().unwrap();
                s.compile_shader(sh);
            }
        })?,
    )?;
    ns.set(
        "glGetShaderParameter",
        Function::new(ctx.clone(), {
            let store = store.clone();
            move |id: i32, sh: u32, pname: u32| -> Option<bool> {
                let s = surface!(store, id);
                let g = s.lock().unwrap();
                if pname != gl::COMPILE_STATUS {
                    return Some(false);
                }
                g.get_shader_compile_ok(sh)
            }
        })?,
    )?;
    ns.set(
        "glGetShaderInfoLog",
        Function::new(ctx.clone(), {
            let store = store.clone();
            move |id: i32, sh: u32| -> String {
                let s = surface!(store, id);
                let g = s.lock().unwrap();
                g.get_shader_info_log(sh)
            }
        })?,
    )?;
    ns.set(
        "glCreateProgram",
        Function::new(ctx.clone(), {
            let store = store.clone();
            move |id: i32| -> u32 {
                let s = surface!(store, id);
                let mut s = s.lock().unwrap();
                s.create_program()
            }
        })?,
    )?;
    ns.set(
        "glAttachShader",
        Function::new(ctx.clone(), {
            let store = store.clone();
            move |id: i32, p: u32, sh: u32| {
                let s = surface!(store, id);
                let mut s = s.lock().unwrap();
                s.attach_shader(p, sh);
            }
        })?,
    )?;
    ns.set(
        "glLinkProgram",
        Function::new(ctx.clone(), {
            let store = store.clone();
            move |id: i32, p: u32| {
                let s = surface!(store, id);
                let mut s = s.lock().unwrap();
                s.link_program(p);
            }
        })?,
    )?;
    ns.set(
        "glGetProgramParameter",
        Function::new(ctx.clone(), {
            let store = store.clone();
            move |id: i32, p: u32, pname: u32| -> Option<bool> {
                let s = surface!(store, id);
                let g = s.lock().unwrap();
                if pname != gl::LINK_STATUS {
                    return Some(false);
                }
                g.get_program_link_ok(p)
            }
        })?,
    )?;
    ns.set(
        "glGetProgramInfoLog",
        Function::new(ctx.clone(), {
            let store = store.clone();
            move |id: i32, p: u32| -> String {
                let s = surface!(store, id);
                let g = s.lock().unwrap();
                g.get_program_info_log(p)
            }
        })?,
    )?;
    ns.set(
        "glGetAttribLocation",
        Function::new(ctx.clone(), {
            let store = store.clone();
            move |id: i32, p: u32, name: String| -> Option<i32> {
                let s = surface!(store, id);
                let g = s.lock().unwrap();
                g.get_attrib_location(p, &name)
            }
        })?,
    )?;
    ns.set(
        "glGetUniformLocation",
        Function::new(ctx.clone(), {
            let store = store.clone();
            move |id: i32, p: u32, name: String| -> Option<i32> {
                let s = surface!(store, id);
                let g = s.lock().unwrap();
                g.get_uniform_location(p, &name)
            }
        })?,
    )?;
    ns.set(
        "glCreateTexture",
        Function::new(ctx.clone(), {
            let store = store.clone();
            move |id: i32| -> u32 {
                let s = surface!(store, id);
                let mut s = s.lock().unwrap();
                s.create_texture()
            }
        })?,
    )?;
    ns.set(
        "glCreateVertexArray",
        Function::new(ctx.clone(), {
            let store = store.clone();
            move |id: i32| -> u32 {
                let s = surface!(store, id);
                let mut s = s.lock().unwrap();
                s.create_vertex_array()
            }
        })?,
    )?;

    // ---- buffers / VAOs -----------------------------------------------------
    ns.set(
        "glBindBuffer",
        Function::new(ctx.clone(), {
            let store = store.clone();
            move |id: i32, target: u32, b: u32| {
                let s = surface!(store, id);
                let mut s = s.lock().unwrap();
                s.bind_buffer(target, b);
            }
        })?,
    )?;
    ns.set(
        "glBufferData",
        Function::new(ctx.clone(), {
            let store = store.clone();
            move |id: i32, target: u32, data: Value| {
                let Some(bytes) = bytes(&data) else { return };
                let s = surface!(store, id);
                let mut s = s.lock().unwrap();
                s.buffer_data(target, &bytes);
            }
        })?,
    )?;
    ns.set(
        "glBufferSubData",
        Function::new(ctx.clone(), {
            let store = store.clone();
            move |id: i32, target: u32, offset: u32, data: Value| {
                let Some(bytes) = bytes(&data) else { return };
                let s = surface!(store, id);
                let mut s = s.lock().unwrap();
                s.buffer_sub_data(target, offset, &bytes);
            }
        })?,
    )?;
    ns.set(
        "glBindVertexArray",
        Function::new(ctx.clone(), {
            let store = store.clone();
            move |id: i32, vao: u32| {
                let s = surface!(store, id);
                let mut s = s.lock().unwrap();
                s.bind_vertex_array(vao);
            }
        })?,
    )?;
    ns.set(
        "glEnableVertexAttribArray",
        Function::new(ctx.clone(), {
            let store = store.clone();
            move |id: i32, idx: u32| {
                let s = surface!(store, id);
                let mut s = s.lock().unwrap();
                s.enable_vertex_attrib_array(idx);
            }
        })?,
    )?;
    ns.set(
        "glVertexAttribPointer",
        Function::new(ctx.clone(), {
            let store = store.clone();
            move |id: i32, idx: u32, size: i32, typ: u32, norm: bool, stride: i32, off: u32| {
                let s = surface!(store, id);
                let mut s = s.lock().unwrap();
                s.vertex_attrib_pointer(idx, size, typ, norm, stride, off);
            }
        })?,
    )?;

    // ---- uniforms -------------------------------------------------------------
    ns.set(
        "glUniform1f",
        Function::new(ctx.clone(), {
            let store = store.clone();
            move |id: i32, loc: i32, x: f64| {
                let s = surface!(store, id);
                let mut s = s.lock().unwrap();
                s.uniform_1f(loc, x);
            }
        })?,
    )?;
    ns.set(
        "glUniform2f",
        Function::new(ctx.clone(), {
            let store = store.clone();
            move |id: i32, loc: i32, x: f64, y: f64| {
                let s = surface!(store, id);
                let mut s = s.lock().unwrap();
                s.uniform_2f(loc, x, y);
            }
        })?,
    )?;
    ns.set(
        "glUniform3f",
        Function::new(ctx.clone(), {
            let store = store.clone();
            move |id: i32, loc: i32, x: f64, y: f64, z: f64| {
                let s = surface!(store, id);
                let mut s = s.lock().unwrap();
                s.uniform_3f(loc, x, y, z);
            }
        })?,
    )?;
    ns.set(
        "glUniform4f",
        Function::new(ctx.clone(), {
            let store = store.clone();
            move |id: i32, loc: i32, x: f64, y: f64, z: f64, w: f64| {
                let s = surface!(store, id);
                let mut s = s.lock().unwrap();
                s.uniform_4f(loc, x, y, z, w);
            }
        })?,
    )?;
    ns.set(
        "glUniform1i",
        Function::new(ctx.clone(), {
            let store = store.clone();
            move |id: i32, loc: i32, x: i32| {
                let s = surface!(store, id);
                let mut s = s.lock().unwrap();
                s.uniform_1i(loc, x);
            }
        })?,
    )?;
    ns.set(
        "glUniformFv",
        Function::new(ctx.clone(), {
            let store = store.clone();
            move |id: i32, loc: i32, data: Value| {
                let Some(vals) = f32s(&data) else { return };
                let s = surface!(store, id);
                let mut s = s.lock().unwrap();
                s.uniform_fv(loc, &vals);
            }
        })?,
    )?;

    // ---- textures ---------------------------------------------------------------
    ns.set(
        "glActiveTexture",
        Function::new(ctx.clone(), {
            let store = store.clone();
            move |id: i32, unit: u32| {
                let s = surface!(store, id);
                let mut s = s.lock().unwrap();
                s.active_texture(unit);
            }
        })?,
    )?;
    ns.set(
        "glBindTexture",
        Function::new(ctx.clone(), {
            let store = store.clone();
            move |id: i32, target: u32, t: u32| {
                let s = surface!(store, id);
                let mut s = s.lock().unwrap();
                s.bind_texture(target, t);
            }
        })?,
    )?;
    ns.set(
        "glTexParameteri",
        Function::new(ctx.clone(), {
            let store = store.clone();
            move |id: i32, target: u32, name: u32, value: u32| {
                let s = surface!(store, id);
                let mut s = s.lock().unwrap();
                s.tex_parameteri(target, name, value);
            }
        })?,
    )?;
    ns.set(
        "glTexImage2DBytes",
        Function::new(ctx.clone(), {
            let store = store.clone();
            move |id: i32, w: f64, h: f64, data: Value, rgb: bool| {
                let Some(px) = bytes(&data) else { return };
                let s = surface!(store, id);
                let mut s = s.lock().unwrap();
                let (w, h) = (w.max(1.0) as u32, h.max(1.0) as u32);
                let flipped = if s.flip_y() { s.flip_rows(w, h, &px) } else { px };
                s.tex_image_2d_bytes(w, h, &flipped, rgb);
            }
        })?,
    )?;
    ns.set(
        "glTexImage2DCanvas",
        Function::new(ctx.clone(), {
            let store = store.clone();
            move |id: i32, src_node: i32| -> bool {
                // Pull the current pixels of a Canvas2D surface (same realm).
                let Some((w, h, rgba)) = crate::platform::canvas_pixels(&canvas2d_store, src_node)
                else {
                    return false;
                };
                let s = surface!(store, id);
                let mut s = s.lock().unwrap();
                s.tex_image_2d_canvas(w, h, &rgba);
                true
            }
        })?,
    )?;
    ns.set(
        "glPixelStoreiFlipY",
        Function::new(ctx.clone(), {
            let store = store.clone();
            move |id: i32, flip: bool| {
                let s = surface!(store, id);
                let mut s = s.lock().unwrap();
                s.pixel_storei_flip_y(flip);
            }
        })?,
    )?;

    // ---- state -----------------------------------------------------------------
    ns.set(
        "glEnable",
        Function::new(ctx.clone(), {
            let store = store.clone();
            move |id: i32, cap: u32| {
                let s = surface!(store, id);
                let mut s = s.lock().unwrap();
                s.enable(cap);
            }
        })?,
    )?;
    ns.set(
        "glDisable",
        Function::new(ctx.clone(), {
            let store = store.clone();
            move |id: i32, cap: u32| {
                let s = surface!(store, id);
                let mut s = s.lock().unwrap();
                s.disable(cap);
            }
        })?,
    )?;
    ns.set(
        "glBlendFunc",
        Function::new(ctx.clone(), {
            let store = store.clone();
            move |id: i32, src: u32, dst: u32| {
                let s = surface!(store, id);
                let mut s = s.lock().unwrap();
                s.blend_func_separate(src, dst, src, dst);
            }
        })?,
    )?;
    ns.set(
        "glDepthFunc",
        Function::new(ctx.clone(), {
            let store = store.clone();
            move |id: i32, f: u32| {
                let s = surface!(store, id);
                let mut s = s.lock().unwrap();
                s.depth_func(f);
            }
        })?,
    )?;
    ns.set(
        "glDepthMask",
        Function::new(ctx.clone(), {
            let store = store.clone();
            move |id: i32, on: bool| {
                let s = surface!(store, id);
                let mut s = s.lock().unwrap();
                s.depth_mask(on);
            }
        })?,
    )?;
    ns.set(
        "glFrontFace",
        Function::new(ctx.clone(), {
            let store = store.clone();
            move |id: i32, f: u32| {
                let s = surface!(store, id);
                let mut s = s.lock().unwrap();
                s.front_face(f);
            }
        })?,
    )?;
    ns.set(
        "glCullFace",
        Function::new(ctx.clone(), {
            let store = store.clone();
            move |id: i32, f: u32| {
                let s = surface!(store, id);
                let mut s = s.lock().unwrap();
                s.cull_face(f);
            }
        })?,
    )?;
    ns.set(
        "glViewport",
        Function::new(ctx.clone(), {
            let store = store.clone();
            move |id: i32, x: i32, y: i32, w: i32, h: i32| {
                let s = surface!(store, id);
                let mut s = s.lock().unwrap();
                s.viewport(x, y, w, h);
            }
        })?,
    )?;
    ns.set(
        "glScissor",
        Function::new(ctx.clone(), {
            let store = store.clone();
            move |id: i32, x: i32, y: i32, w: i32, h: i32| {
                let s = surface!(store, id);
                let mut s = s.lock().unwrap();
                s.scissor(x, y, w, h);
            }
        })?,
    )?;
    ns.set(
        "glClearColor",
        Function::new(ctx.clone(), {
            let store = store.clone();
            move |id: i32, r: f64, g: f64, b: f64, a: f64| {
                let s = surface!(store, id);
                let mut s = s.lock().unwrap();
                s.clear_color(r, g, b, a);
            }
        })?,
    )?;
    ns.set(
        "glClearDepth",
        Function::new(ctx.clone(), {
            let store = store.clone();
            move |id: i32, d: f64| {
                let s = surface!(store, id);
                let mut s = s.lock().unwrap();
                s.clear_depth(d);
            }
        })?,
    )?;
    ns.set(
        "glUseProgram",
        Function::new(ctx.clone(), {
            let store = store.clone();
            move |id: i32, p: u32| {
                let s = surface!(store, id);
                let mut s = s.lock().unwrap();
                s.use_program(p);
            }
        })?,
    )?;
    ns.set(
        "glClear",
        Function::new(ctx.clone(), {
            let store = store.clone();
            move |id: i32, mask: u32| {
                let s = surface!(store, id);
                let mut s = s.lock().unwrap();
                s.clear(mask);
            }
        })?,
    )?;
    ns.set(
        "glGetError",
        Function::new(ctx.clone(), {
            let store = store.clone();
            move |id: i32| -> u32 {
                let s = surface!(store, id);
                let mut s = s.lock().unwrap();
                s.get_error()
            }
        })?,
    )?;

    // ---- draw --------------------------------------------------------------------
    ns.set(
        "glDrawArrays",
        Function::new(ctx.clone(), {
            let store = store.clone();
            move |id: i32, mode: u32, first: i32, count: i32| {
                let s = surface!(store, id);
                let mut s = s.lock().unwrap();
                s.draw(mode, first, count, false, 0);
            }
        })?,
    )?;
    ns.set(
        "glDrawElements",
        Function::new(ctx.clone(), {
            let store = store.clone();
            move |id: i32, mode: u32, count: i32, typ: u32, offset: i32| {
                let s = surface!(store, id);
                let mut s = s.lock().unwrap();
                // offset is in BYTES per spec; convert to element index.
                let elem_size = if typ == gl::UNSIGNED_INT {
                    4
                } else {
                    2
                };
                let first = offset / elem_size;
                s.draw(mode, first, count, true, typ);
            }
        })?,
    )?;
    Ok(())
}
