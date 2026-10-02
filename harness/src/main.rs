//! # brows — the Brows12 headless CLI test harness
//!
//! Runs the FULL engine pipeline (network → privacy → parse → style →
//! layout → paint → scripts → canvas harvest → compositor) with **no GUI**:
//!
//! ```text
//! brows render --url https://example.com --png page.png --json page.json
//! brows render --html page.html --png local.png --animate 1200
//! brows suite  --list sites.txt --dir out/ --json results.json
//! ```
//!
//! Exit codes: 0 = page loaded + rendered, 2 = pipeline error.

use std::sync::Arc;
use std::time::Instant;

use brows12_api::prelude::*;
use brows12_compositor::{Backend, CompositeStats};

#[derive(Debug, Clone)]
struct Args {
    #[allow(dead_code)]
    url: Option<String>,
    html: Option<String>,
    png: Option<String>,
    json: Option<String>,
    width: u32,
    height: u32,
    #[allow(dead_code)]
    timeout_ms: u64,
    animate_ms: u64,
    scroll_y: f32,
    quiet: bool,
}

fn arg(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).map(|s| s.to_string())
}

fn flag(args: &[String], name: &str) -> bool {
    args.iter().any(|a| a == name)
}

fn main() {
    let raw: Vec<String> = std::env::args().skip(1).collect();
    let cmd = raw.first().map(|s| s.as_str()).unwrap_or("help").to_string();
    let rest: Vec<String> = raw.iter().skip(1).cloned().collect();
    match cmd.as_str() {
        "render" => run_render(rest),
        "suite" => run_suite(rest),
        _ => print_help(),
    }
}

fn print_help() {
    println!(
        r#"brows — Brows12 headless engine harness (v0.2)

USAGE:
  brows render --url <url>     [--png out.png] [--json out.json] [--width W] [--height H]
               [--html file]   [--animate ms] [--scroll y] [--quiet]
  brows suite  --list urls.txt [--dir out/] [--json results.json] [--width W] [--height H]

EXAMPLES:
  brows render --url https://example.com --png example.png --json example.json
  brows render --html fixtures/test.html --png test.png --animate 1000
  brows suite --list sites.txt --dir shots/ --json results.json"#
    );
}

struct LoadOutcome {
    loaded: bool,
    error: Option<String>,
    console: Vec<String>,
    title: String,
    final_url: String,
    png: Option<Vec<u8>>,
    load_ms: u128,
    stats: Option<CompositeStats>,
    scroll_y: f32,
}

fn engine_and_tab(width: u32, height: u32) -> (Browser, Arc<Tab>, ConsoleRx) {
    let browser = Browser::builder()
        .viewport(width, height)
        .user_agent("Brows12/0.2 (brows harness)")
        .max_live_pages(4)
        .privacy(|p| {
            p.block_ads = true;
            p.https_upgrade = true;
            p.block_cname_cloaking = true;
        })
        .build();
    let console_rx = browser.subscribe();
    let tab = browser.new_tab();
    (browser, tab, console_rx)
}

// Type alias to keep the subscription receiver name readable.
type ConsoleRx = tokio::sync::broadcast::Receiver<EngineEvent>;

fn drain_console(rx: &mut ConsoleRx, tab_id: u64) -> Vec<String> {
    let mut out = Vec::new();
    while let Ok(event) = rx.try_recv() {
        if let EngineEvent::Console { tab, message } = event {
            if tab == tab_id {
                out.push(message);
            }
        }
    }
    out
}

fn measure_load(tab: &Arc<Tab>, console_rx: &mut ConsoleRx, args: &Args) -> LoadOutcome {
    let t0 = Instant::now();
    let load_result = if let Some(ref html) = args.html {
        let content = std::fs::read_to_string(html)
            .unwrap_or_else(|e| panic!("cannot read --html {html}: {e}"));
        tab.load_url_from_string(&content, "brows12://fixture/")
    } else {
        tab.load_url(args.url.as_deref().unwrap_or("about:blank"))
    };
    let loaded = load_result.is_ok();
    let error = load_result.err().map(|e| e.to_string());
    let load_ms = t0.elapsed().as_millis();

    if args.animate_ms > 0 {
        let _ = tab.advance_animation(args.animate_ms, 16);
    }
    if args.scroll_y > 0.0 {
        let _ = tab.set_scroll(args.scroll_y);
    }

    let frame = tab.frame();
    let png = frame.as_ref().and_then(|f| f.encode_png().ok());
    let stats = tab.compositor_stats();

    LoadOutcome {
        loaded,
        error,
        console: drain_console(console_rx, tab.id()),
        title: tab.title(),
        final_url: tab.url(),
        png,
        load_ms,
        stats,
        scroll_y: tab.scroll_y(),
    }
}

fn stats_to_json(stats: &Option<CompositeStats>) -> serde_json::Value {
    match stats {
        None => serde_json::Value::Null,
        Some(s) => {
            let backend = match &s.backend {
                Backend::Gpu { adapter, api } => {
                    serde_json::json!({ "kind": "gpu", "adapter": adapter, "api": api })
                }
                Backend::Cpu => serde_json::json!({ "kind": "cpu" }),
            };
            serde_json::json!({
                "backend": backend,
                "frame_time_us": s.frame_time_us,
                "layer_count": s.layer_count,
                "texture_bytes": s.texture_bytes,
            })
        }
    }
}

fn json_report(args: &Args, out: &LoadOutcome) -> serde_json::Value {
    serde_json::json!({
        "engine": format!("brows12 v{}", env!("CARGO_PKG_VERSION")),
        "input": args.url.clone().or_else(|| args.html.clone()).unwrap_or_default(),
        "loaded": out.loaded,
        "error": out.error,
        "title": out.title,
        "final_url": out.final_url,
        "viewport": { "width": args.width, "height": args.height },
        "timing": { "full_pipeline_ms": out.load_ms },
        "render": {
            "png_bytes": out.png.as_ref().map(|p| p.len()).unwrap_or(0),
            "scroll_y": out.scroll_y,
            "compositor": stats_to_json(&out.stats),
        },
        "console": out.console,
        "screenshot": args.png.clone(),
    })
}

fn write_png(path: &Option<String>, png: &Option<Vec<u8>>) {
    if let (Some(path), Some(bytes)) = (path, png) {
        if let Some(parent) = std::path::Path::new(path).parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        std::fs::write(path, bytes).expect("write png");
    }
}

fn write_json(path: &Option<String>, value: &serde_json::Value) {
    if let Some(path) = path {
        if let Some(parent) = std::path::Path::new(path).parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        std::fs::write(path, serde_json::to_string_pretty(value).unwrap_or_else(|_| "{}".into()))
            .expect("write json");
    }
}

fn run_render(rest: Vec<String>) {
    let args = Args {
        url: arg(&rest, "--url"),
        html: arg(&rest, "--html"),
        png: arg(&rest, "--png"),
        json: arg(&rest, "--json"),
        width: arg(&rest, "--width").and_then(|v| v.parse().ok()).unwrap_or(1280),
        height: arg(&rest, "--height").and_then(|v| v.parse().ok()).unwrap_or(800),
        timeout_ms: arg(&rest, "--timeout").and_then(|v| v.parse().ok()).unwrap_or(20000),
        animate_ms: arg(&rest, "--animate").and_then(|v| v.parse().ok()).unwrap_or(0),
        scroll_y: arg(&rest, "--scroll").and_then(|v| v.parse().ok()).unwrap_or(0.0),
        quiet: flag(&rest, "--quiet"),
    };
    let (_browser, tab, mut console_rx) = engine_and_tab(args.width, args.height);
    let outcome = measure_load(&tab, &mut console_rx, &args);
    write_png(&args.png, &outcome.png);
    let report = json_report(&args, &outcome);
    write_json(&args.json, &report);
    if !args.quiet {
        println!("{}", serde_json::to_string_pretty(&report).unwrap_or_default());
    }
    if !outcome.loaded || outcome.png.is_none() {
        std::process::exit(2);
    }
}

fn run_suite(rest: Vec<String>) {
    let list_path = arg(&rest, "--list").unwrap_or_else(|| "sites.txt".into());
    let dir = arg(&rest, "--dir").unwrap_or_else(|| "shots".into());
    let suite_json = arg(&rest, "--json").unwrap_or_else(|| "results.json".into());
    let width = arg(&rest, "--width").and_then(|v| v.parse().ok()).unwrap_or(1280);
    let height = arg(&rest, "--height").and_then(|v| v.parse().ok()).unwrap_or(800);

    let urls: Vec<String> = std::fs::read_to_string(&list_path)
        .expect("suite url list")
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| l.to_string())
        .collect();
    std::fs::create_dir_all(&dir).expect("create suite dir");

    let mut results = Vec::new();
    for url in &urls {
        let name = slug(url);
        let t0 = Instant::now();
        let (_browser, tab, mut console_rx) = engine_and_tab(width, height);
        let outcome = measure_load(
            &tab,
            &mut console_rx,
            &Args {
                url: Some(url.clone()),
                html: None,
                png: None,
                json: None,
                width,
                height,
                timeout_ms: 20000,
                animate_ms: 0,
                scroll_y: 0.0,
                quiet: true,
            },
        );
        let png_path = format!("{dir}/{name}.png");
        write_png(&Some(png_path.clone()), &outcome.png);
        let elapsed = t0.elapsed();
        results.push(serde_json::json!({
            "url": url,
            "loaded": outcome.loaded,
            "error": outcome.error,
            "title": outcome.title,
            "screenshot": format!("{name}.png"),
            "png_bytes": outcome.png.as_ref().map(|p| p.len()).unwrap_or(0),
            "total_ms": elapsed.as_millis() as u64,
            "compositor": stats_to_json(&outcome.stats),
        }));
        println!(
            "{} {url} title={:?} png={}b {}ms",
            if outcome.loaded { "OK " } else { "ERR" },
            outcome.title,
            outcome.png.as_ref().map(|p| p.len()).unwrap_or(0),
            elapsed.as_millis()
        );
    }

    std::fs::write(
        &suite_json,
        serde_json::to_string_pretty(&serde_json::json!({
            "engine": env!("CARGO_PKG_VERSION"),
            "count": results.len(),
            "results": results,
        }))
        .unwrap_or_default(),
    )
    .expect("write suite json");
}

fn slug(url: &str) -> String {
    let clean: String = url
        .replace("https://", "")
        .replace("http://", "")
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .collect();
    clean.trim_matches('_').to_string()
}
