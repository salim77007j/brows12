//! x11driver — drives brows12-ui under Xvfb for validation:
//! finds the window, injects real X11 input (XTEST), screenshots the root
//! window, and polls the window title (the shell reports url+status there).
//!
//! Usage:
//!   x11driver wininfo
//!   x11driver shot <out.png>
//!   x11driver click <x> <y>
//!   x11driver type <text>
//!   x11driver key <Return|BackSpace|Escape|F5>
//!   x11driver wheel <dy>
//!   x11driver waittitle <substr> <timeout_ms>
//!   x11driver waitidle <timeout_ms>

use std::process::exit;
use std::time::{Duration, Instant};

use x11rb::connection::Connection;
use x11rb::protocol::xproto::*;

use x11rb::rust_connection::RustConnection;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("usage: x11driver <wininfo|shot|click|type|key|wheel|waittitle|waitidle> ...");
        exit(2);
    }
    let (conn, screen) = RustConnection::connect(None).expect("connect to X display");
    let root = conn.setup().roots[screen].root;
    let cmd = args[1].as_str();

    match cmd {
        "wininfo" => {
            let w = find_window(&conn, root).expect("brows12 window not found");
            let g = conn.get_geometry(w).unwrap().reply().unwrap();
            let t = conn.translate_coordinates(w, root, 0, 0).unwrap().reply().unwrap();
            println!("win {} at {},{} size {}x{}", w, t.dst_x, t.dst_y, g.width, g.height);
        }
        "shot" => {
            let out = args.get(2).expect("shot <out.png>");
            let g = conn.get_geometry(root).unwrap().reply().unwrap();
            let img = conn
                .get_image(ImageFormat::Z_PIXMAP, root, 0, 0, g.width, g.height, !0u32)
                .unwrap()
                .reply()
                .unwrap();
            // Depth-24 ZPixmap is BGRX in memory; convert to RGBA.
            let mut rgba = Vec::with_capacity(img.data.len() / 3 * 4);
            for px in img.data.chunks_exact(4) {
                rgba.extend_from_slice(&[px[2], px[1], px[0], 255]);
            }
            image::RgbaImage::from_raw(g.width as u32, g.height as u32, rgba)
                .expect("image buffer")
                .save(out)
                .expect("save png");
            println!("saved {}", out);
        }
        "click" => {
            let w = find_window(&conn, root).expect("brows12 window not found");
            let (ox, oy) = origin(&conn, w);
            let x: i32 = args[2].parse().expect("x");
            let y: i32 = args[3].parse().expect("y");
            let (ax, ay) = (ox + x, oy + y);
            warp(&conn, root, ax, ay);
            conn.set_input_focus(x11rb::protocol::xproto::InputFocus::NONE, w, 0u32).unwrap();
            click_button(&conn, 1);
            conn.flush().unwrap();
        }
        "probe" => {
            let f = conn.get_input_focus().unwrap().reply().unwrap();
            println!("focus window: {}", f.focus);
            let tree = conn.query_tree(root).unwrap().reply().unwrap();
            for w in tree.children {
                let g = conn.get_geometry(w).unwrap().reply().unwrap();
                println!("win {} geom {}x{} name {:?}", w, g.width, g.height, window_title(&conn, w));
            }
        }
        "dumpmap" => {
            let map = keymap(&conn);
            let mut v: Vec<_> = map.iter().filter(|(k, _)| (**k >= 0x20 && **k < 0x7f) || **k == 0xff0d || **k == 0xff08).collect();
            v.sort_by_key(|(k, _)| **k);
            for (ks, (kc, sh)) in v.iter().take(80) {
                let ch = if **ks >= 0x20 && **ks < 0x7f { format!("{}", **ks as u8 as char) } else { format!("U+{:04x}", **ks) };
                println!("{ch} -> kc={} shift={}", kc, sh);
            }
        }
        "focus" => {
            let w = find_window(&conn, root).expect("brows12 window not found");
            conn.set_input_focus(x11rb::protocol::xproto::InputFocus::NONE, w, 0u32).unwrap();
            conn.flush().unwrap();
            println!("focused {w}");
        }
        "move" => {
            let w = find_window(&conn, root).expect("brows12 window not found");
            let (ox, oy) = origin(&conn, w);
            let (ax, ay) = (ox + args[2].parse::<i32>().unwrap(), oy + args[3].parse::<i32>().unwrap());
            warp(&conn, root, ax, ay);
            conn.flush().unwrap();
        }
        "type" => {
            let text = args.get(2).cloned().unwrap_or_default();
            let map = keymap(&conn);
            for ch in text.chars() {
                let (kc, shift) = *map
                    .get(&(ch as u32))
                    .unwrap_or_else(|| panic!("no keycode for {:?}", ch));
                if shift {
                    press_key(&conn, map[&0xffe1u32].0);
                }
                press_key(&conn, kc);
                if shift {
                    release_key(&conn, map[&0xffe1u32].0);
                }
            }
            conn.flush().unwrap();
        }
        "key" => {
            let name = args[2].as_str();
            let keysym = match name {
                "Return" | "Enter" => 0xff0d,
                "BackSpace" => 0xff08,
                "Escape" => 0xff1b,
                "F5" => 0xffc8,
                "space" => 0x20,
                other => panic!("unknown key {other}"),
            };
            let map = keymap(&conn);
            let (kc, _) = *map.get(&keysym).unwrap_or_else(|| panic!("key {name} not mapped"));
            press_key(&conn, kc);
            conn.flush().unwrap();
        }
        "wheel" => {
            let dy: f32 = args[2].parse().unwrap();
            let n = (dy.abs() / 60.0).ceil().max(1.0) as u32;
            let btn = if dy > 0.0 { 5 } else { 4 };
            for _ in 0..n {
                click_button(&conn, btn);
                std::thread::sleep(Duration::from_millis(40));
            }
            conn.flush().unwrap();
        }
        "waittitle" => {
            let substr = args.get(2).cloned().unwrap_or_default();
            let timeout: u64 = args.get(3).map(|s| s.parse().unwrap()).unwrap_or(60000);
            let w = find_window(&conn, root).expect("brows12 window not found");
            let start = Instant::now();
            loop {
                let title = window_title(&conn, w);
                if title.contains(&substr) {
                    println!("title matched: {title}");
                    return;
                }
                if start.elapsed() > Duration::from_millis(timeout) {
                    eprintln!("TIMEOUT waiting for title containing {substr:?}; last: {title:?}");
                    exit(1);
                }
                std::thread::sleep(Duration::from_millis(150));
            }
        }
        "waitidle" => {
            // Wait until the title stops containing the loading marker.
            let timeout: u64 = args.get(2).map(|s| s.parse().unwrap()).unwrap_or(90000);
            let w = find_window(&conn, root).expect("brows12 window not found");
            let start = Instant::now();
            loop {
                let title = window_title(&conn, w);
                let loading = title.contains("Loading");
                if !loading {
                    println!("idle: {title}");
                    return;
                }
                if start.elapsed() > Duration::from_millis(timeout) {
                    eprintln!("TIMEOUT waiting for idle; last: {title:?}");
                    exit(1);
                }
                std::thread::sleep(Duration::from_millis(150));
            }
        }
        other => {
            eprintln!("unknown command {other}");
            exit(2);
        }
    }
}

fn find_window(conn: &RustConnection, root: Window) -> Option<Window> {
    let tree = conn.query_tree(root).unwrap().reply().unwrap();
    for w in tree.children {
        if window_title(conn, w).contains("brows12") {
            return Some(w);
        }
    }
    None
}

fn window_title(conn: &RustConnection, w: Window) -> String {
    if let Ok(reply) = conn.get_property(false, w, AtomEnum::WM_NAME, AtomEnum::STRING, 0, 2048) {
        if let Ok(p) = reply.reply() {
            return String::from_utf8_lossy(&p.value).into_owned();
        }
    }
    String::new()
}

fn origin(conn: &RustConnection, w: Window) -> (i32, i32) {
    let root = conn.setup().roots[0].root;
    let t = conn.translate_coordinates(w, root, 0, 0).unwrap().reply().unwrap();
    (t.dst_x as i32, t.dst_y as i32)
}

fn warp(conn: &RustConnection, root: Window, x: i32, y: i32) {
    conn.warp_pointer(root, root, 0, 0, 0, 0, x as i16, y as i16).unwrap().check().ok();
    std::thread::sleep(Duration::from_millis(60));
}

fn click_button(conn: &RustConnection, button: u8) {
    fake(conn, X_BUTTON_PRESS, button);
    std::thread::sleep(Duration::from_millis(50));
    fake(conn, X_BUTTON_RELEASE, button);
    std::thread::sleep(Duration::from_millis(50));
}

const X_KEY_PRESS: u8 = 2;
const X_KEY_RELEASE: u8 = 3;
const X_BUTTON_PRESS: u8 = 4;
const X_BUTTON_RELEASE: u8 = 5;

fn fake(conn: &RustConnection, type_: u8, detail: u8) {
    x11rb::protocol::xtest::fake_input(conn, type_, detail, 0, 0, 0, 0, 0).unwrap();
    std::thread::sleep(Duration::from_millis(25));
}

fn press_key(conn: &RustConnection, kc: u8) {
    fake(conn, X_KEY_PRESS, kc);
    fake(conn, X_KEY_RELEASE, kc);
}

fn release_key(conn: &RustConnection, kc: u8) {
    fake(conn, X_KEY_RELEASE, kc);
}

/// Map keysym → (keycode, needs_shift) from the server's keyboard mapping.
fn keymap(conn: &RustConnection) -> std::collections::HashMap<u32, (u8, bool)> {
    let setup = conn.setup();
    let min = setup.min_keycode;
    let max = setup.max_keycode;
    let count = (max - min + 1) as u32;
    let mapping = conn.get_keyboard_mapping(min, max - min + 1).unwrap().reply().unwrap();
    let per = mapping.keysyms_per_keycode as usize;
    let mut map = std::collections::HashMap::new();
    for i in 0..count as usize {
        let kc = min + i as u8;
        for col in 0..per {
            let ks = mapping.keysyms[i * per + col];
            if ks == 0 {
                continue;
            }
            let entry = map.entry(ks).or_insert((kc, col >= 2));
            if col == 0 {
                *entry = (kc, false);
            } else if col == 1 {
                *entry = (kc, true);
            }
        }
    }
    map
}
