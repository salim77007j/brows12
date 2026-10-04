//! Chrome drawing + hit testing: tab strip, toolbar (back/forward/reload +
//! omnibox + status), and the webview viewport blit. Everything is drawn
//! into one tiny-skia pixmap each frame; text uses the shell's own
//! cosmic-text rasterizer.

use crate::model::{Status, UiTab};
use crate::text::UiText;
use tiny_skia::{Color, FillRule, Paint, PathBuilder, Pixmap, Stroke, Transform};

// ---- Layout constants (logical pixels) -------------------------------
pub const WIN_W: u32 = crate::model::VIEWPORT_W;
pub const WIN_H: u32 = crate::model::VIEWPORT_H + 72;

pub const STRIP_H: f32 = 32.0; // tab strip
pub const TOOL_H: f32 = 40.0; // toolbar
pub const CHROME_H: f32 = STRIP_H + TOOL_H;

pub const TAB_X0: f32 = 8.0;
pub const TAB_W: f32 = 200.0;
pub const TAB_GAP: f32 = 4.0;

pub const BTN_BACK_X: f32 = 8.0;
pub const BTN_FWD_X: f32 = 48.0;
pub const BTN_RELOAD_X: f32 = 88.0;
pub const BTN_SIZE: f32 = 32.0;
pub const OMNI_X: f32 = 130.0;
pub const OMNI_RIGHT_PAD: f32 = 158.0;

// ---- Colors (straight RGBA byte tuples) ------------------------------
const STRIP_BG: [u8; 4] = [0xc8, 0xcd, 0xd4, 0xff];
const TOOL_BG: [u8; 4] = [0xe8, 0xea, 0xee, 0xff];
const TAB_ACTIVE: [u8; 4] = [0xff, 0xff, 0xff, 0xff];
const TAB_INACTIVE: [u8; 4] = [0xdc, 0xdf, 0xe4, 0xff];
const INK: [u8; 4] = [0x1b, 0x27, 0x33, 0xff];
const INK_DIM: [u8; 4] = [0x5a, 0x6b, 0x7c, 0xff];
/// Title color for hibernated (suspended) tabs — dimmest tier.
const INK_SUSPENDED: [u8; 4] = [0x9a, 0xa5, 0xb1, 0xff];
const OMNI_BG: [u8; 4] = [0xff, 0xff, 0xff, 0xff];
const OMNI_BORDER: [u8; 4] = [0xb9, 0xc0, 0xc9, 0xff];
const VIEWPORT_BG: [u8; 4] = [0x22, 0x22, 0x26, 0xff];
const BTN_HOVER: [u8; 4] = [0xd4, 0xd8, 0xde, 0xff];
const CLOSE_BG: [u8; 4] = [0xe3, 0xe6, 0xea, 0xff];
const ERROR_RED: [u8; 4] = [0xb3, 0x3b, 0x3b, 0xff];
const LOADING_AMBER: [u8; 4] = [0xa0, 0x6a, 0x00, 0xff];

fn col(c: [u8; 4]) -> Color {
    Color::from_rgba8(c[0], c[1], c[2], c[3])
}

// ---- Hit testing -----------------------------------------------------
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Hit {
    None,
    Tab(u32),
    TabClose(u32),
    NewTab,
    Back,
    Forward,
    Reload,
    Omnibox,
}

/// Tab-strip geometry for hit tests (mirrors the drawing code).
fn tab_rect(i: u32) -> (f32, f32, f32, f32) {
    let x = TAB_X0 + i as f32 * (TAB_W + TAB_GAP);
    (x, 3.0, TAB_W, STRIP_H - 6.0)
}

pub fn hit_test(x: f32, y: f32, tab_count: usize) -> Hit {
    if y < STRIP_H {
        for i in 0..tab_count as u32 {
            let (tx, ty, tw, th) = tab_rect(i);
            let cx = tx + tw - 20.0;
            if x >= cx && x <= tx + tw && y >= ty && y <= ty + th {
                return Hit::TabClose(i);
            }
            if x >= tx && x <= tx + tw && y >= ty && y <= ty + th {
                return Hit::Tab(i);
            }
        }
        let px = TAB_X0 + tab_count as f32 * (TAB_W + TAB_GAP);
        if x >= px && x <= px + 24.0 && (4.0..=28.0).contains(&y) {
            return Hit::NewTab;
        }
        return Hit::None;
    }
    if y < CHROME_H {
        if (36.0..=68.0).contains(&y) {
            if (BTN_BACK_X..=BTN_BACK_X + BTN_SIZE).contains(&x) && y >= 38.0 {
                return Hit::Back;
            }
            if (BTN_FWD_X..=BTN_FWD_X + BTN_SIZE).contains(&x) {
                return Hit::Forward;
            }
            if (BTN_RELOAD_X..=BTN_RELOAD_X + BTN_SIZE).contains(&x) {
                return Hit::Reload;
            }
            if x >= OMNI_X && x <= WIN_W as f32 - OMNI_RIGHT_PAD {
                return Hit::Omnibox;
            }
        }
        return Hit::None;
    }
    Hit::None
}

// ---- Drawing ---------------------------------------------------------

/// Rounded-rectangle path (tiny-skia 0.12 has no fill_rrect helper).
fn rrect_path(x: f32, y: f32, w: f32, h: f32, r: f32) -> Option<tiny_skia::Path> {
    let r = r.min(w / 2.0).min(h / 2.0);
    let mut pb = PathBuilder::new();
    pb.move_to(x + r, y);
    pb.line_to(x + w - r, y);
    pb.cubic_to(x + w, y, x + w, y, x + w, y + r);
    pb.line_to(x + w, y + h - r);
    pb.cubic_to(x + w, y + h, x + w, y + h, x + w - r, y + h);
    pb.line_to(x + r, y + h);
    pb.cubic_to(x, y + h, x, y + h, x, y + h - r);
    pb.line_to(x, y + r);
    pb.cubic_to(x, y, x, y, x + r, y);
    pb.close();
    pb.finish()
}

fn fill(px: &mut Pixmap, x: f32, y: f32, w: f32, h: f32, color: [u8; 4], radius: f32) {
    let mut paint = Paint::default();
    paint.set_color(col(color));
    paint.anti_alias = radius > 0.0;
    if radius > 0.0 {
        if let Some(path) = rrect_path(x, y, w, h, radius) {
            px.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);
        }
    } else {
        px.fill_rect(
            tiny_skia::Rect::from_xywh(x, y, w, h).expect("rect"),
            &paint,
            Transform::identity(),
            None,
        );
    }
}

/// Rasterize `text` with the shell's cosmic-text rasterizer and blit it.
#[allow(clippy::too_many_arguments)]
fn text(
    px: &mut Pixmap,
    raster: &mut UiText,
    s: &str,
    x: f32,
    y: f32,
    size: f32,
    color: [u8; 4],
    max_w: f32,
) {
    if let Some(glyphs) = raster.rasterize_text(s, size, 400, false, None, color, Some(max_w)) {
        px.draw_pixmap(
            x as i32,
            y as i32,
            glyphs.as_ref(),
            &tiny_skia::PixmapPaint::default(),
            Transform::identity(),
            None,
        );
    }
}

fn stroke_shape(px: &mut Pixmap, build: impl FnOnce(&mut PathBuilder), width: f32, color: [u8; 4]) {
    let mut pb = PathBuilder::new();
    build(&mut pb);
    if let Some(path) = pb.finish() {
        let mut paint = Paint::default();
        paint.set_color(col(color));
        paint.anti_alias = true;
        let stroke = Stroke {
            width,
            line_cap: tiny_skia::LineCap::Round,
            line_join: tiny_skia::LineJoin::Round,
            ..Stroke::default()
        };
        px.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
    }
}

fn measure(raster: &mut UiText, s: &str, size: f32) -> f32 {
    raster.measure(s, size)
}

/// Draw the whole chrome onto `px`. `group_colors` is aligned with
/// `tabs`: the group color bar drawn at each tab's top edge (4.4.3).
pub fn draw_chrome(
    px: &mut Pixmap,
    raster: &mut UiText,
    tabs: &[UiTab],
    active: usize,
    hover: Hit,
    caret_on: bool,
    group_colors: &[Option<[u8; 3]>],
) {
    let w = px.width() as f32;

    // Tab strip + toolbar backgrounds.
    fill(px, 0.0, 0.0, w, STRIP_H, STRIP_BG, 0.0);
    fill(px, 0.0, STRIP_H, w, TOOL_H, TOOL_BG, 0.0);

    // Tabs.
    for (i, t) in tabs.iter().enumerate() {
        let (tx, ty, tw, th) = tab_rect(i as u32);
        let is_active = i == active;
        fill(px, tx, ty, tw, th, if is_active { TAB_ACTIVE } else { TAB_INACTIVE }, 6.0);
        // Phase 4.4.3: group color bar along the tab's top edge.
        if let Some(Some(rgb)) = group_colors.get(i) {
            let gcol = [rgb[0], rgb[1], rgb[2], 0xff];
            fill(px, tx + 4.0, ty, tw - 8.0, 3.0, gcol, 1.5);
        }
        let title = if t.title.is_empty() { t.url() } else { t.title.clone() };
        let title: String = title.chars().take(24).collect();
        let ink = if is_active {
            INK
        } else if t.suspended {
            INK_SUSPENDED
        } else {
            INK_DIM
        };
        text(px, raster, &title, tx + 8.0, ty + 7.0, 12.0, ink, tw - 34.0);
        // Close button.
        fill(px, tx + tw - 20.0, ty + 4.0, 16.0, 16.0, CLOSE_BG, 4.0);
        stroke_shape(
            px,
            |pb| {
                pb.move_to(tx + tw - 20.0 + 4.5, ty + 8.5);
                pb.line_to(tx + tw - 20.0 + 11.5, ty + 15.5);
                pb.move_to(tx + tw - 20.0 + 11.5, ty + 8.5);
                pb.line_to(tx + tw - 20.0 + 4.5, ty + 15.5);
            },
            1.6,
            INK_DIM,
        );
        // Phase 4.4.2: memory-discard marker — a small red dot on the
        // tab's lower-left corner tells the user this tab was discarded
        // by the governor (not closed); it reloads on activation.
        if t.discarded && t.suspended {
            let (cx, cy, r) = (tx + 9.0, ty + th - 6.0, 2.6);
            // tiny-skia's from_circle already returns Option<Path>.
            if let Some(path) = PathBuilder::from_circle(cx, cy, r) {
                let mut p = Paint::default();
                p.set_color(col(ERROR_RED));
                p.anti_alias = true;
                px.fill_path(&path, &p, FillRule::Winding, Transform::identity(), None);
            }
        }
        // Phase 4.4.6: pinned marker — a small accent bar on the tab's
        // left edge (pinned tabs never hibernate and persist sessions).
        if t.pinned {
            fill(px, tx, ty + 6.0, 3.0, th - 12.0, [0x2b, 0x6c, 0xb0, 0xff], 1.5);
        }
    }

    // "+" new-tab button.
    let nx = TAB_X0 + tabs.len() as f32 * (TAB_W + TAB_GAP);
    fill(px, nx, 4.0, 24.0, 24.0, CLOSE_BG, 5.0);
    fill(px, nx + 10.0, 9.0, 4.0, 14.0, INK_DIM, 1.0);
    fill(px, nx + 5.0, 14.0, 14.0, 4.0, INK_DIM, 1.0);

    // Nav buttons.
    if hover == Hit::Back {
        fill(px, BTN_BACK_X, 38.0, BTN_SIZE, BTN_SIZE, BTN_HOVER, 8.0);
    }
    stroke_shape(
        px,
        |pb| {
            pb.move_to(BTN_BACK_X + 20.0, 46.0);
            pb.line_to(BTN_BACK_X + 12.0, 54.0);
            pb.line_to(BTN_BACK_X + 20.0, 62.0);
        },
        2.5,
        INK,
    );

    if hover == Hit::Forward {
        fill(px, BTN_FWD_X, 38.0, BTN_SIZE, BTN_SIZE, BTN_HOVER, 8.0);
    }
    stroke_shape(
        px,
        |pb| {
            pb.move_to(BTN_FWD_X + 12.0, 46.0);
            pb.line_to(BTN_FWD_X + 20.0, 54.0);
            pb.line_to(BTN_FWD_X + 12.0, 62.0);
        },
        2.5,
        INK,
    );

    if hover == Hit::Reload {
        fill(px, BTN_RELOAD_X, 38.0, BTN_SIZE, BTN_SIZE, BTN_HOVER, 8.0);
    }
    draw_reload(px);

    // Omnibox.
    let ow = w - OMNI_X - OMNI_RIGHT_PAD;
    fill(px, OMNI_X, 38.0, ow, 28.0, OMNI_BG, 14.0);
    fill(px, OMNI_X + 1.0, 38.0, ow - 2.0, 1.0, OMNI_BORDER, 0.0);
    fill(px, OMNI_X + 1.0, 65.0, ow - 2.0, 1.0, OMNI_BORDER, 0.0);
    fill(px, OMNI_X, 39.0, 1.0, 26.0, OMNI_BORDER, 0.0);
    fill(px, OMNI_X + ow - 1.0, 39.0, 1.0, 26.0, OMNI_BORDER, 0.0);

    let active_tab = &tabs[active];
    let editing = active_tab.omni_edit.is_some();
    let omni_text = active_tab.omni_edit.clone().unwrap_or_else(|| active_tab.url());
    text(px, raster, &omni_text, OMNI_X + 14.0, 45.0, 13.0, INK, ow - 28.0);
    if editing && caret_on {
        let cw = measure(raster, &omni_text, 13.0);
        let cx = (OMNI_X + 14.0 + cw).min(OMNI_X + ow - 16.0);
        fill(px, cx, 43.0, 1.5, 18.0, INK, 0.0);
    }

    // Status (right side of the toolbar).
    let status = active_tab.status.label();
    if !status.is_empty() {
        let sw = measure(raster, &status, 12.0).min(OMNI_RIGHT_PAD - 16.0);
        let color = match active_tab.status {
            Status::Error(_) => ERROR_RED,
            Status::Loading => LOADING_AMBER,
            _ => INK_DIM,
        };
        text(px, raster, &status, w - 12.0 - sw, 47.0, 12.0, color, OMNI_RIGHT_PAD - 16.0);
    }

    // Viewport backdrop (visible until the page frame arrives).
    fill(px, 0.0, CHROME_H, w, px.height() as f32 - CHROME_H, VIEWPORT_BG, 0.0);
}

/// Circular-arrow reload glyph centered in the reload button.
fn draw_reload(px: &mut Pixmap) {
    let cx = BTN_RELOAD_X + BTN_SIZE / 2.0;
    let cy = 38.0 + BTN_SIZE / 2.0;
    let r = 8.0;
    // 320° arc with the gap on the right.
    let a0 = -330f32.to_radians();
    let a1 = 10f32.to_radians();
    let steps = 26;
    stroke_shape(
        px,
        |pb| {
            for s in 0..=steps {
                let a = a0 + (a1 - a0) * (s as f32 / steps as f32);
                let x = cx + r * a.cos();
                let y = cy - r * a.sin();
                if s == 0 {
                    pb.move_to(x, y);
                } else {
                    pb.line_to(x, y);
                }
            }
        },
        2.5,
        INK,
    );
    // Arrowhead at the arc end.
    let mut ah = PathBuilder::new();
    ah.move_to(cx + r + 4.5, cy - 5.0);
    ah.line_to(cx + r - 0.5, cy - 0.5);
    ah.line_to(cx + r + 5.5, cy + 1.5);
    ah.close();
    if let Some(path) = ah.finish() {
        let mut paint = Paint::default();
        paint.set_color(col(INK));
        paint.anti_alias = true;
        px.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);
    }
}
