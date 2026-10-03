//! `brows-perf` — the Phase 2 benchmark CLI. Loads URLs in headless Servo
//! tabs and reports memory / startup / idle-CPU / scroll numbers as JSON.
//!
//! Examples:
//!   brows-perf --url https://example.com/
//!   brows-perf --url https://example.com/ --url https://en.wikipedia.org/wiki/Rust \
//!       --suspend --idle-secs 20 --scroll-secs 5 --json /tmp/perf.json

use std::path::PathBuf;

use servo_host::perf::{run_perf, PerfConfig};

fn main() {
    let mut cfg = PerfConfig::default();
    let mut json: Option<PathBuf> = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--url" => {
                let u = args.next().expect("--url needs a value");
                cfg.urls
                    .push(url::Url::parse(&u).unwrap_or_else(|e| panic!("bad url {u}: {e}")));
            },
            "--width" => cfg.width = args.next().expect("value").parse().expect("u32"),
            "--height" => cfg.height = args.next().expect("value").parse().expect("u32"),
            "--settle-ms" => cfg.settle_ms = args.next().expect("value").parse().expect("u64"),
            "--timeout-ms" => cfg.timeout_ms = args.next().expect("value").parse().expect("u64"),
            "--suspend" => cfg.suspend = true,
            "--idle-secs" => cfg.idle_secs = args.next().expect("value").parse().expect("u64"),
            "--scroll-secs" => {
                cfg.scroll_secs = args.next().expect("value").parse().expect("u64")
            },
            "--json" => json = Some(PathBuf::from(args.next().expect("value"))),
            "--help" | "-h" => {
                eprintln!("{}", __doc__().replace("//! ", ""));
                std::process::exit(0);
            },
            other => panic!("unknown arg {other}"),
        }
    }
    // Default: a single example.com tab (replace the built-in default).
    if std::env::args().len() == 1 {
        cfg.urls = vec![url::Url::parse("https://example.com/").unwrap()];
    }

    let report = run_perf(cfg);
    let text = serde_json::to_string_pretty(&report).expect("serialize perf report");
    println!("{text}");
    if let Some(path) = &json {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(path, &text);
        eprintln!("[brows-perf] json written to {}", path.display());
    }
}

fn __doc__() -> &'static str {
    "//! `brows-perf` — Phase 2 benchmark CLI: memory / startup / idle-CPU / scroll.\n//! Flags: --url U (repeatable) --width --height --settle-ms --timeout-ms\n//!        --suspend --idle-secs N --scroll-secs N --json PATH"
}
