//! Engine-vs-browser comparison harness.
//!
//! Measures engine-side timings that map 1:1 to user-visible operations:
//! cold start (engine construction), tab creation, and first paint of a
//! synthetic page. Results print as JSON + a markdown table.
//!
//! For cross-browser comparisons run `benchmarks/scripts/compare_browsers.sh`
//! (requires Chrome/Firefox/Brave installed locally).

use anyhow::Result;
use std::time::Instant;

fn main() -> Result<()> {
    let mut results = Vec::new();

    // Cold start: engine construction (runtime + TLS stack + font scan).
    let t0 = Instant::now();
    let engine = brows12_engine::Engine::new(Default::default());
    results.push(("cold_start_engine_ms", t0.elapsed().as_millis() as f64));

    // Tab creation.
    let t0 = Instant::now();
    let _tab = engine.tab();
    results.push(("tab_create_ms", t0.elapsed().as_millis() as f64));

    // First paint of an internal page (no network).
    let tab = engine.tab();
    let t0 = Instant::now();
    tab.load_url_from_string(
        "<!DOCTYPE html><html><head><title>About</title></head><body><h1>Brows12</h1><p>Cold start page.</p></body></html>",
        "brows12://about",
    )?;
    results.push(("internal_page_first_paint_ms", t0.elapsed().as_millis() as f64));

    // Local-network page load (loopback HTTP/1.1, includes full pipeline).
    let (addr, handle) = spawn_server();
    let tab2 = engine.tab();
    let t0 = Instant::now();
    tab2.load_url(&format!("http://{addr}/"))?;
    results.push(("local_page_full_pipeline_ms", t0.elapsed().as_millis() as f64));
    drop(handle);

    // JSON output.
    println!("{}", serde_json::to_string_pretty(&results.iter().map(|(k, v)| (k.to_string(), *v)).collect::<std::collections::BTreeMap<_, _>>())?);

    // Markdown table.
    println!("\n| metric | ms |\n|---|---|");
    for (k, v) in &results {
        println!("| {k} | {v:.2} |");
    }
    Ok(())
}

fn spawn_server() -> (std::net::SocketAddr, std::thread::JoinHandle<()>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = std::thread::spawn(move || {
        for stream in listener.incoming() {
            let mut sock = match stream {
                Ok(s) => s,
                Err(_) => return,
            };
            use std::io::{Read, Write};
            let mut buf = [0u8; 4096];
            let _ = sock.read(&mut buf);
            let body = "<html><head><title>Bench</title></head><body><h1>hello</h1><p>world</p></body></html>";
            let _ = sock.write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{}", body.len(), body).as_bytes());
        }
    });
    (addr, handle)
}
