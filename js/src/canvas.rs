//! Canvas 2D: page-script surfaces painted with tiny-skia.
//!
//! Surfaces live in a [`CanvasStore`] keyed by DOM node id. After scripts
//! run, the engine harvests surfaces whose node is a live `<canvas>` and
//! routes them into the image pipeline (layout + paint see them like any
//! decoded image). This is the same compositor-ready path `<img>` uses.

use std::sync::Mutex;
use tiny_skia::Pixmap;

/// One path segment, already in untransformed user space.
#[derive(Debug, Clone, Copy)]
pub enum PathSeg {
    MoveTo(f32, f32),
    LineTo(f32, f32),
    Arc { cx: f32, cy: f32, r: f32, start: f32, end: f32, ccw: bool },
    Close,
}

/// Drawing state per surface (spec-shaped subset).
#[derive(Debug, Clone)]
pub struct CanvasState {
    pub fill: [u8; 4],
    pub stroke: [u8; 4],
    pub line_width: f32,
    pub global_alpha: f32,
    /// Affine [a, b, c, d, e, f] (x' = a*x + c*y + e, y' = b*x + d*y + f).
    pub transform: [f32; 6],
    pub path: Vec<PathSeg>,
    pub font_size: f32,
    pub font_weight: u16,
    pub font_italic: bool,
    pub font_family: String,
}

impl Default for CanvasState {
    fn default() -> Self {
        CanvasState {
            fill: [0, 0, 0, 255],
            stroke: [0, 0, 0, 255],
            line_width: 1.0,
            global_alpha: 1.0,
            transform: [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
            path: Vec::new(),
            font_size: 10.0,
            font_weight: 400,
            font_italic: false,
            font_family: "sans-serif".into(),
        }
    }
}

/// A page-owned canvas surface.
pub struct CanvasSurface {
    pub width: u32,
    pub height: u32,
    pub pixmap: Mutex<Pixmap>,
    pub state: Mutex<CanvasState>,
}

/// Registry of canvas surfaces for one page realm.
#[derive(Default)]
pub struct CanvasStore {
    canvases: Mutex<std::collections::HashMap<i32, std::sync::Arc<CanvasSurface>>>,
}

impl CanvasStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get_or_create(
        &self,
        node_id: i32,
        width: u32,
        height: u32,
    ) -> std::sync::Arc<CanvasSurface> {
        let mut map = self.canvases.lock().unwrap();
        if let Some(existing) = map.get(&node_id) {
            // Same size: reuse. Resize resets the surface (spec behaviour).
            if existing.width == width && existing.height == height {
                return existing.clone();
            }
        }

        {
            let surface = std::sync::Arc::new(CanvasSurface {
                width,
                height,
                pixmap: Mutex::new(
                    Pixmap::new(width.max(1), height.max(1))
                        .unwrap_or_else(|| Pixmap::new(1, 1).unwrap()),
                ),
                state: Mutex::new(CanvasState::default()),
            });
            map.insert(node_id, surface.clone());
            surface
        }
    }

    pub fn remove(&self, node_id: i32) {
        self.canvases.lock().unwrap().remove(&node_id);
    }

    /// Drop all surfaces (navigation: node ids of the previous document must
    /// not leak surfaces into the next one).
    pub fn clear(&self) {
        self.canvases.lock().unwrap().clear();
    }

    /// Snapshot of all surfaces for engine harvesting.
    pub fn snapshot(&self) -> Vec<(i32, std::sync::Arc<CanvasSurface>)> {
        self.canvases.lock().unwrap().iter().map(|(k, v)| (*k, v.clone())).collect()
    }
}

/// Parse a CSS color string (#hex, rgb(), rgba(), named basics) to RGBA8.
pub fn parse_css_color(s: &str) -> Option<[u8; 4]> {
    let s = s.trim();
    let named: Option<[u8; 4]> = match s.to_ascii_lowercase().as_str() {
        "black" => Some([0, 0, 0, 255]),
        "white" => Some([255, 255, 255, 255]),
        "red" => Some([255, 0, 0, 255]),
        "green" => Some([0, 128, 0, 255]),
        "lime" => Some([0, 255, 0, 255]),
        "blue" => Some([0, 0, 255, 255]),
        "yellow" => Some([255, 255, 0, 255]),
        "cyan" | "aqua" => Some([0, 255, 255, 255]),
        "magenta" | "fuchsia" => Some([255, 0, 255, 255]),
        "gray" | "grey" => Some([128, 128, 128, 255]),
        "orange" => Some([255, 165, 0, 255]),
        "purple" => Some([128, 0, 128, 255]),
        "transparent" => Some([0, 0, 0, 0]),
        _ => None,
    };
    if let Some(n) = named {
        return Some(n);
    }
    if let Some(hex) = s.strip_prefix('#') {
        let (r, g, b, a) = match hex.len() {
            3 => (
                u8::from_str_radix(&hex[0..1].repeat(2), 16).ok()?,
                u8::from_str_radix(&hex[1..2].repeat(2), 16).ok()?,
                u8::from_str_radix(&hex[2..3].repeat(2), 16).ok()?,
                255u8,
            ),
            6 => (
                u8::from_str_radix(&hex[0..2], 16).ok()?,
                u8::from_str_radix(&hex[2..4], 16).ok()?,
                u8::from_str_radix(&hex[4..6], 16).ok()?,
                255u8,
            ),
            8 => (
                u8::from_str_radix(&hex[0..2], 16).ok()?,
                u8::from_str_radix(&hex[2..4], 16).ok()?,
                u8::from_str_radix(&hex[4..6], 16).ok()?,
                u8::from_str_radix(&hex[6..8], 16).ok()?,
            ),
            _ => return None,
        };
        return Some([r, g, b, a]);
    }
    // rgb(a) functional forms.
    let body = s
        .trim_start_matches("rgba")
        .trim_start_matches("rgb")
        .trim()
        .strip_prefix('(')?
        .strip_suffix(')')?;
    let parts: Vec<&str> = body.split(',').map(|p| p.trim()).collect();
    if parts.len() < 3 {
        return None;
    }
    let f = |v: &str| v.trim_end_matches('%').parse::<f32>().ok();
    let r = f(parts[0])? as u8;
    let g = f(parts[1])? as u8;
    let b = f(parts[2])? as u8;
    let a = if parts.len() >= 4 {
        let av = f(parts[3])?;
        if parts[3].trim().ends_with('%') {
            (av / 100.0 * 255.0) as u8
        } else {
            (av * 255.0) as u8
        }
    } else {
        255
    };
    Some([r, g, b, a])
}

/// Premultiplied -> straight alpha RGBA8 (engine image pipeline contract).
pub fn canvas_unpremultiply(data: &[u8]) -> Vec<u8> {
    let mut out = data.to_vec();
    for px in out.chunks_exact_mut(4) {
        let a = px[3];
        if a == 0 {
            px[0] = 0;
            px[1] = 0;
            px[2] = 0;
        } else if a != 255 {
            let a16 = a as u16;
            px[0] = ((px[0] as u16 * 255 + a16 / 2) / a16).min(255) as u8;
            px[1] = ((px[1] as u16 * 255 + a16 / 2) / a16).min(255) as u8;
            px[2] = ((px[2] as u16 * 255 + a16 / 2) / a16).min(255) as u8;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn color_parsing() {
        assert_eq!(parse_css_color("#ff0000"), Some([255, 0, 0, 255]));
        assert_eq!(parse_css_color("#0f0"), Some([0, 255, 0, 255]));
        assert_eq!(parse_css_color("rgba(255, 0, 0, 0.5)"), Some([255, 0, 0, 127]));
        assert_eq!(parse_css_color("blue"), Some([0, 0, 255, 255]));
        assert_eq!(parse_css_color("transparent"), Some([0, 0, 0, 0]));
        assert_eq!(parse_css_color("not-a-color"), None);
    }

    #[test]
    fn store_resizes() {
        let store = CanvasStore::new();
        let c = store.get_or_create(7, 300, 150);
        assert_eq!(c.width, 300);
        let _ = store.get_or_create(7, 640, 480);
        let again = store.get_or_create(7, 640, 480);
        assert_eq!(again.width, 640);
    }
}
