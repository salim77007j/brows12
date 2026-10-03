//! brows-servo — headless render CLI over the Servo host.
//!
//! Usage:
//!   brows-servo --url https://example.com --png out.png --json out.json
//!   brows-servo --html page.html --png out.png

use servo_host::{run_headless, HeadlessConfig};

fn arg_value(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let url_arg = arg_value(&args, "--url");
    let html_arg = arg_value(&args, "--html");

    let url: url::Url = if let Some(html) = html_arg {
        let body = std::fs::read(&html).expect("read --html file");
        format!("data:text/html;base64,{}", use_base64(&body))
            .parse()
            .expect("data url")
    } else if let Some(u) = url_arg {
        u.parse().expect("parse --url")
    } else {
        eprintln!("brows-servo: need --url or --html");
        std::process::exit(2);
    };

    let config = HeadlessConfig {
        url,
        width: arg_value(&args, "--width")
            .and_then(|v| v.parse().ok())
            .unwrap_or(1280),
        height: arg_value(&args, "--height")
            .and_then(|v| v.parse().ok())
            .unwrap_or(800),
        png: arg_value(&args, "--png").map(std::path::PathBuf::from),
        json: arg_value(&args, "--json").map(std::path::PathBuf::from),
        timeout_ms: arg_value(&args, "--timeout-ms")
            .and_then(|v| v.parse().ok())
            .unwrap_or(60_000),
        settle_ms: arg_value(&args, "--settle-ms")
            .and_then(|v| v.parse().ok())
            .unwrap_or(1_200),
    };

    let report = run_headless(config);
    println!("{}", serde_json::to_string(&report).unwrap_or_default());
    if report.crashed || report.error.is_some() {
        std::process::exit(1);
    }
}

fn use_base64(data: &[u8]) -> String {
    // Minimal base64 (standard alphabet, padding) to avoid a dependency.
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            T[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            T[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}
