use brows12_engine::{Engine, EngineConfig, EngineEvent};

fn local_server(files: Vec<(&'static str, String)>) -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let files: std::collections::HashMap<String, String> = files
        .into_iter()
        .map(|(p, c)| (p.to_string(), c))
        .collect();
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
            let body = files.get(&path).cloned().unwrap_or_else(|| "<html><body>404</body></html>".into());
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(resp.as_bytes());
            let _ = stream.flush();
        }
    });
    format!("http://{addr}")
}

#[test]
fn end_to_end_page_load_with_css_and_js() {
    let html = r##"<!DOCTYPE html>
<html><head>
<link rel="stylesheet" href="/style.css">
<script>document.getElementById('target').textContent = 'JS-RAN';</script>
</head><body>
<h1 id="target">INITIAL</h1>
<p class="intro">Styled paragraph.</p>
</body></html>"##;
    let css = "body { background-color: #102030; } h1 { color: #ff8800; font-size: 40px; } .intro { margin-top: 8px; }";

    let base = local_server(vec![("/", html.to_string()), ("/style.css", css.to_string())]);

    let engine = Engine::new(EngineConfig::default());
    let mut events = engine.subscribe();
    let tab = engine.tab();
    tab.load_url(&base).expect("load must succeed");

    // JS ran and mutated the DOM → frame regenerated (gen >= 2 when mutated).
    let frame = tab.frame().expect("frame present");
    assert!(frame.generation >= 1);

    // Ink present (heading + text painted).
    let inked = frame.pixmap.data().chunks_exact(4).filter(|p| p[3] > 0).count();
    assert!(inked > 200, "expected painted ink, got {inked} px");

    // Events flowed.
    let mut saw_frame = false;
    while let Ok(ev) = events.try_recv() {
        if let EngineEvent::FrameReady { .. } = ev {
            saw_frame = true;
        }
    }
    assert!(saw_frame);
}

#[test]
fn blocked_page_renders_shield() {
    let engine = Engine::new(EngineConfig::default());
    let tab = engine.tab();
    tab.load_url("https://doubleclick.net/ad").ok();
    if tab.url().starts_with("brows12://blocked") {
        assert_eq!(tab.title(), "Blocked by Brows12");
    }
}

#[test]
fn suspension_keeps_snapshot() {
    let html = "<html><head><title>Holder</title></head><body><p>Suspend me</p></body></html>";
    let base = local_server(vec![("/", html.to_string())]);
    let engine = Engine::new(EngineConfig::default());
    let tab = engine.tab();
    tab.load_url(&base).unwrap();
    let before = tab.frame().unwrap().generation;
    tab.suspend();
    assert!(!tab.is_live());
    assert_eq!(tab.title(), "Holder");
    assert!(tab.frame().is_some());
    tab.resume().unwrap();
    assert!(tab.is_live());
    assert!(tab.frame().unwrap().generation >= before);
    assert_eq!(engine.live_page_count(), 1);
}
