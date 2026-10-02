//! Canvas 2D native bindings + platform APIs (performance.now, WASM).
//!
//! All canvas drawing operates on [`CanvasSurface`]s from the page's
//! [`CanvasStore`]. Transforms are applied CPU-side (points mapped through
//! the affine matrix before path construction) — tiny-skia then rasterizes
//! with the same premultiplied pipeline the compositor consumes.

use crate::canvas::{parse_css_color, CanvasState, CanvasStore, CanvasSurface, PathSeg};
use crate::environment::JsEnvironment;
use rquickjs::{Ctx, Function, Object};
use std::sync::Arc;
use tiny_skia::Pixmap;

impl CanvasSurface {
    pub(crate) fn map_point(state: &CanvasState, x: f32, y: f32) -> (f32, f32) {
        let t = &state.transform;
        (t[0] * x + t[2] * y + t[4], t[1] * x + t[3] * y + t[5])
    }

    pub(crate) fn build_path(state: &CanvasState) -> Option<tiny_skia::Path> {
        let mut pb = tiny_skia::PathBuilder::new();
        for seg in &state.path {
            match seg {
                PathSeg::MoveTo(x, y) => {
                    let (tx, ty) = Self::map_point(state, *x, *y);
                    pb.move_to(tx, ty);
                }
                PathSeg::LineTo(x, y) => {
                    let (tx, ty) = Self::map_point(state, *x, *y);
                    pb.line_to(tx, ty);
                }
                PathSeg::Arc { cx, cy, r, start, end, ccw } => {
                    let steps = 32.max(((end - start).abs() / 0.2) as usize);
                    for i in 0..=steps {
                        let t = i as f32 / steps as f32;
                        let ang = if *ccw {
                            *end + t * (*start - *end)
                        } else {
                            *start + t * (*end - *start)
                        };
                        let (px, py) = (cx + r * ang.cos(), cy + r * ang.sin());
                        let (tx, ty) = Self::map_point(state, px, py);
                        if i == 0 {
                            pb.move_to(tx, ty);
                        } else {
                            pb.line_to(tx, ty);
                        }
                    }
                }
                PathSeg::Close => pb.close(),
            }
        }
        pb.finish()
    }
}

/// Install the `__brows12` canvas + platform natives.
pub fn install<'js>(
    ctx: &Ctx<'js>,
    env: &Arc<JsEnvironment>,
    ns: &Object<'js>,
) -> rquickjs::Result<()> {
    let store = env.canvas_store.clone();

    // ---- surface lifecycle -------------------------------------------------
    ns.set(
        "canvasEnsure",
        Function::new(ctx.clone(), {
            let store = store.clone();
            move |id: i32, w: f64, h: f64| {
                store.get_or_create(id, w.max(0.0) as u32, h.max(0.0) as u32);
                true
            }
        })?,
    )?;
    ns.set(
        "canvasSetSize",
        Function::new(ctx.clone(), {
            let store = store.clone();
            move |id: i32, w: f64, h: f64| {
                // Recreate the surface (spec: resizing clears the canvas).
                let surface = store.get_or_create(id, w.max(1.0) as u32, h.max(1.0) as u32);
                let w_px = surface.width.max(1);
                let h_px = surface.height.max(1);
                if let Some(fresh) = Pixmap::new(w_px, h_px) {
                    let mut guard = match surface.pixmap.lock() {
                        Ok(g) => g,
                        Err(_) => return,
                    };
                    *guard = fresh;
                }
            }
        })?,
    )?;
    ns.set(
        "canvasGetSize",
        Function::new(ctx.clone(), {
            let store = store.clone();
            move |id: i32| -> Vec<f64> {
                store
                    .snapshot()
                    .into_iter()
                    .find(|(k, _)| *k == id)
                    .map(|(_, s)| vec![s.width as f64, s.height as f64])
                    .unwrap_or_else(|| vec![300.0, 150.0])
            }
        })?,
    )?;

    // ---- state -------------------------------------------------------------
    ns.set(
        "canvasSetFillStyle",
        Function::new(ctx.clone(), {
            let store = store.clone();
            move |id: i32, color: String| {
                if let Some(surface) = find(&store, id) {
                    if let Ok(mut st) = surface.state.lock() {
                        if let Some(rgba) = parse_css_color(&color) {
                            st.fill = rgba;
                        }
                    }
                }
            }
        })?,
    )?;
    ns.set(
        "canvasSetStrokeStyle",
        Function::new(ctx.clone(), {
            let store = store.clone();
            move |id: i32, color: String| {
                if let Some(surface) = find(&store, id) {
                    if let Ok(mut st) = surface.state.lock() {
                        if let Some(rgba) = parse_css_color(&color) {
                            st.stroke = rgba;
                        }
                    }
                }
            }
        })?,
    )?;
    ns.set(
        "canvasSetState",
        Function::new(ctx.clone(), {
            let store = store.clone();
            move |id: i32, json: String| {
                use serde_json::Value;
                let Ok(v) = serde_json::from_str::<Value>(&json) else { return };
                let g = |k: &str| v.get(k).and_then(Value::as_f64).unwrap_or(0.0);
                let s = (
                    g("lineWidth"),
                    g("alpha"),
                    g("a"),
                    g("b"),
                    g("c"),
                    g("d"),
                    g("e"),
                    g("f"),
                    g("fontSize"),
                    g("fontWeight"),
                    v.get("italic").and_then(Value::as_bool).unwrap_or(false),
                    v.get("family").and_then(Value::as_str).unwrap_or("sans-serif").to_string(),
                );
                if let Some(surface) = find(&store, id) {
                    if let Ok(mut st) = surface.state.lock() {
                        st.line_width = s.0.max(0.0) as f32;
                        st.global_alpha = s.1.clamp(0.0, 1.0) as f32;
                        st.transform = [
                            s.2 as f32, s.3 as f32, s.4 as f32, s.5 as f32, s.6 as f32, s.7 as f32,
                        ];
                        st.font_size = s.8.max(1.0) as f32;
                        st.font_weight = (s.9 as i32).clamp(1, 1000) as u16;
                        st.font_italic = s.10;
                        st.font_family = s.11;
                    }
                }
            }
        })?,
    )?;
    // ---- paths -------------------------------------------------------------
    ns.set(
        "canvasPathOp",
        Function::new(ctx.clone(), {
            let store = store.clone();
            move |id: i32, op: i32, x: f64, y: f64, z: f64, w: f64| {
                if let Some(surface) = find(&store, id) {
                    if let Ok(mut st) = surface.state.lock() {
                        match op {
                            0 => st.path.push(PathSeg::MoveTo(x as f32, y as f32)),
                            1 => st.path.push(PathSeg::LineTo(x as f32, y as f32)),
                            2 => st.path.push(PathSeg::Arc {
                                cx: x as f32,
                                cy: y as f32,
                                r: z.max(0.0) as f32,
                                start: 0.0,
                                end: w as f32,
                                ccw: false,
                            }),
                            3 => st.path.push(PathSeg::Close),
                            4 => st.path.clear(),
                            _ => {}
                        }
                    }
                }
            }
        })?,
    )?;

    // ---- drawing -----------------------------------------------------------
    ns.set(
        "canvasFillRect",
        Function::new(ctx.clone(), {
            let store = store.clone();
            move |id: i32, x: f64, y: f64, w: f64, h: f64, stroke: bool| {
                if let Some(surface) = find(&store, id) {
                    draw_rect(&surface, x as f32, y as f32, w as f32, h as f32, stroke);
                }
            }
        })?,
    )?;
    ns.set(
        "canvasClearRect",
        Function::new(ctx.clone(), {
            let store = store.clone();
            move |id: i32, x: f64, y: f64, w: f64, h: f64| {
                if let Some(surface) = find(&store, id) {
                    if let Ok(mut pm) = surface.pixmap.lock() {
                        let (x, y, w, h) = (x as f32, y as f32, w as f32, h as f32);
                        let r = tiny_skia::Rect::from_xywh(x, y, w.max(0.0), h.max(0.0));
                        if let Some(r) = r {
                            pm.fill_rect(
                                r,
                                &tiny_skia::Paint {
                                    shader: tiny_skia::Shader::SolidColor(
                                        tiny_skia::Color::TRANSPARENT,
                                    ),
                                    ..Default::default()
                                },
                                tiny_skia::Transform::identity(),
                                None,
                            );
                        }
                    }
                }
            }
        })?,
    )?;
    ns.set(
        "canvasFillPath",
        Function::new(ctx.clone(), {
            let store = store.clone();
            move |id: i32, stroke: bool| {
                if let Some(surface) = find(&store, id) {
                    draw_path(&surface, stroke);
                }
            }
        })?,
    )?;
    ns.set(
        "canvasFillText",
        Function::new(ctx.clone(), {
            let env = env.clone();
            let store = store.clone();
            move |id: i32, text: String, x: f64, y: f64, max_width: f64| {
                if let Some(surface) = find(&store, id) {
                    draw_text(
                        &env,
                        &surface,
                        &text,
                        x as f32,
                        y as f32,
                        if max_width > 0.0 { Some(max_width as f32) } else { None },
                    );
                }
            }
        })?,
    )?;
    ns.set(
        "canvasDrawImage",
        Function::new(ctx.clone(), {
            let store = store.clone();
            move |id: i32, src_id: i32, dx: f64, dy: f64, dw: f64, dh: f64| {
                if let (Some(dst), Some(src)) = (find(&store, id), find(&store, src_id)) {
                    if let (Ok(mut pm), Ok(src_pm)) = (dst.pixmap.lock(), src.pixmap.lock()) {
                        let dw = if dw > 0.0 { dw as u32 } else { src.width };
                        let dh = if dh > 0.0 { dh as u32 } else { src.height };
                        if let Some(scaled) = tiny_skia::Pixmap::new(dw.max(1), dh.max(1)) {
                            let mut scaled = scaled;
                            scaled.draw_pixmap(
                                0,
                                0,
                                tiny_skia::PixmapRef::from_bytes(
                                    src_pm.data(),
                                    src.width.max(1),
                                    src.height.max(1),
                                )
                                .unwrap(),
                                &tiny_skia::PixmapPaint::default(),
                                tiny_skia::Transform::from_scale(
                                    dw as f32 / src.width.max(1) as f32,
                                    dh as f32 / src.height.max(1) as f32,
                                ),
                                None,
                            );
                            pm.draw_pixmap(
                                dx as i32,
                                dy as i32,
                                scaled.as_ref(),
                                &tiny_skia::PixmapPaint::default(),
                                tiny_skia::Transform::identity(),
                                None,
                            );
                        }
                    }
                }
            }
        })?,
    )?;
    Ok(())
}

fn find(store: &CanvasStore, id: i32) -> Option<Arc<CanvasSurface>> {
    store.snapshot().into_iter().find(|(k, _)| *k == id).map(|(_, v)| v)
}

fn draw_rect(surface: &CanvasSurface, x: f32, y: f32, w: f32, h: f32, stroke: bool) {
    let Ok(mut pm) = surface.pixmap.lock() else { return };
    let Ok(state) = surface.state.lock() else { return };
    let color = if stroke { state.stroke } else { state.fill };
    let alpha = state.global_alpha;
    let c =
        tiny_skia::Color::from_rgba8(color[0], color[1], color[2], (color[3] as f32 * alpha) as u8);
    let paint = tiny_skia::Paint { shader: tiny_skia::Shader::SolidColor(c), ..Default::default() };
    // Transform the four corners, then build a polygon.
    let corners = [(x, y), (x + w, y), (x + w, y + h), (x, y + h)];
    let mapped: Vec<(f32, f32)> =
        corners.iter().map(|(cx, cy)| CanvasSurface::map_point(&state, *cx, *cy)).collect();
    let mut pb = tiny_skia::PathBuilder::new();
    pb.move_to(mapped[0].0, mapped[0].1);
    for p in &mapped[1..] {
        pb.line_to(p.0, p.1);
    }
    pb.close();
    if let Some(path) = pb.finish() {
        if stroke {
            let stroke =
                tiny_skia::Stroke { width: state.line_width.max(0.1), ..Default::default() };
            pm.stroke_path(&path, &paint, &stroke, tiny_skia::Transform::identity(), None);
        } else {
            pm.fill_path(
                &path,
                &paint,
                tiny_skia::FillRule::Winding,
                tiny_skia::Transform::identity(),
                None,
            );
        }
    }
}

fn draw_path(surface: &CanvasSurface, stroke: bool) {
    let Ok(mut pm) = surface.pixmap.lock() else { return };
    let Ok(state) = surface.state.lock() else { return };
    let color = if stroke { state.stroke } else { state.fill };
    let c = tiny_skia::Color::from_rgba8(
        color[0],
        color[1],
        color[2],
        (color[3] as f32 * state.global_alpha) as u8,
    );
    let paint = tiny_skia::Paint { shader: tiny_skia::Shader::SolidColor(c), ..Default::default() };
    if let Some(path) = CanvasSurface::build_path(&state) {
        if stroke {
            let stroke =
                tiny_skia::Stroke { width: state.line_width.max(0.1), ..Default::default() };
            pm.stroke_path(&path, &paint, &stroke, tiny_skia::Transform::identity(), None);
        } else {
            pm.fill_path(
                &path,
                &paint,
                tiny_skia::FillRule::Winding,
                tiny_skia::Transform::identity(),
                None,
            );
        }
    }
}

fn draw_text(
    env: &std::sync::Arc<JsEnvironment>,
    surface: &CanvasSurface,
    text: &str,
    x: f32,
    y: f32,
    max_width: Option<f32>,
) {
    let Ok(state) = surface.state.lock() else { return };
    let mut rasterizer = brows12_render::raster::Rasterizer::new(env.fonts.clone());
    let Some(glyphs) = rasterizer.rasterize_text(
        text,
        state.font_size,
        state.font_weight,
        state.font_italic,
        Some(&state.font_family),
        state.fill,
        max_width,
    ) else {
        return;
    };
    // Text baseline: y is the alphabetic baseline per spec; the glyph
    // rasterizer anchors at the top, so shift up by the ascent estimate.
    let ascent = state.font_size * 0.8;
    let (tx, ty) = CanvasSurface::map_point(&state, x, y - ascent);
    if let Ok(mut pm) = surface.pixmap.lock() {
        let paint = tiny_skia::PixmapPaint {
            opacity: state.global_alpha.clamp(0.0, 1.0),
            ..Default::default()
        };
        pm.draw_pixmap(
            tx as i32,
            ty as i32,
            glyphs.as_ref(),
            &paint,
            tiny_skia::Transform::identity(),
            None,
        );
    }
}

/// Harvest canvas surfaces into engine image pipeline (straight-alpha RGBA).
pub fn harvest(store: &CanvasStore) -> Vec<(i32, u32, u32, Vec<u8>)> {
    store
        .snapshot()
        .into_iter()
        .map(|(id, surface)| {
            let pixels = if let Ok(pm) = surface.pixmap.lock() {
                crate::canvas::canvas_unpremultiply(pm.data())
            } else {
                Vec::new()
            };
            (id, surface.width, surface.height, pixels)
        })
        .collect()
}
