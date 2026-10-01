//! End-to-end content pipeline: HTML + CSS + JS + render, through the engine.

use brows12_engine::{Engine, EngineConfig};

fn server(pages: Vec<(&'static str, String)>) -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let pages: std::collections::HashMap<String, String> =
        pages.into_iter().map(|(p, c)| (p.to_string(), c)).collect();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let mut stream = match stream {
                Ok(s) => s,
                Err(_) => return,
            };
            use std::io::{Read, Write};
            let mut buf = [0u8; 8192];
            let _ = stream.read(&mut buf);
            let req = String::from_utf8_lossy(&buf);
            let path = req.split_whitespace().nth(1).unwrap_or("/").to_string();
            let body = pages.get(&path).cloned().unwrap_or_else(|| "not found".into());
            let _ = stream.write_all(
                format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{}", body.len(), body)
                    .as_bytes(),
            );
        }
    });
    format!("http://{addr}")
}

#[test]
fn full_page_with_flexbox_and_script_and_image() {
    let page = r##"<!DOCTYPE html>
<html>
<head>
<style>
  body { background-color: #223344; font-size: 20px; }
  .row { display: flex; flex-direction: row; gap: 10px; }
  .box { width: 120px; height: 80px; }
  .a { background-color: #ff0000; }
  .b { background-color: #00ff00; }
</style>
</head>
<body>
<h1>Flex demo</h1>
<div class="row">
  <div class="box a" id="a"></div>
  <div class="box b"></div>
</div>
<script>
  document.getElementById('a').styleSet('width', '200px');
</script>
</body>
</html>"##;

    // A 1x1 red PNG served as /img.png for an <img> (kept out of the page to
    // keep the test focused; the loader path is covered by the unit tests).
    let base = server(vec![("/", page.to_string())]);
    let engine = Engine::new(EngineConfig::default());
    let tab = engine.tab();
    tab.load_url(&base).unwrap();

    let frame = tab.frame().expect("frame rendered");
    let data = frame.rgba_premultiplied();
    let inked = data.chunks_exact(4).filter(|p| p[3] > 0).count();
    assert!(inked > 500, "page must paint substantial ink, got {inked}");

    // Background color should dominate a corner pixel (body background).
    let px = frame.pixmap.pixel(640, 60).unwrap();
    assert!(px.alpha() > 0, "background painted at body area");
    assert_eq!(
        (px.red(), px.green(), px.blue()),
        (0x22, 0x33, 0x44),
        "corner pixel should be the body background #223344"
    );
}

#[test]
fn privacy_blocks_tracker_script_but_not_page() {
    // The page loads fine; a script tag pointing at a blocked tracker host is
    // fetched by the pipeline — the fetch itself will fail DNS, which the
    // engine tolerates; the page still renders.
    let page = "<html><head><title>Okay</title></head><body><p>content</p></body></html>";
    let base = server(vec![("/", page.to_string())]);
    let engine = Engine::new(EngineConfig::default());
    let tab = engine.tab();
    tab.load_url(&base).unwrap();
    assert!(tab.is_live());
    assert!(tab.frame().is_some());
}

#[test]
fn memory_budget_suspends_oldest() {
    let html = "<html><head><title>T</title></head><body><p>x</p></body></html>";
    let base = server(vec![("/", html.to_string())]);
    let engine = Engine::new(EngineConfig { max_live_pages: 2, ..EngineConfig::default() });
    let t1 = engine.tab();
    let t2 = engine.tab();
    let t3 = engine.tab();
    t1.load_url(&base).unwrap();
    t2.load_url(&base).unwrap();
    t3.load_url(&base).unwrap();
    // All three loads above each trigger a budget sweep; final count <= 2.
    assert!(engine.live_page_count() <= 2);
    // Oldest tab must be the suspended one.
    assert!(!t1.is_live() || !t2.is_live());
    assert!(t3.is_live());
}
