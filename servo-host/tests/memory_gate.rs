//! v2.1 Phase 2.6 — memory regression gates (e2e, env-dependent).
//!
//! These tests spawn the built `brows-perf` binary and assert the v2.1
//! memory envelopes. They are `#[ignore]`d by default because they need
//! the release binaries, a working GL stack (Xvfb, `DISPLAY=:99`) and
//! network access; the Linux CI job runs them with `--ignored` (wired
//! in the Phase 4 release workflow). Threshold rationale:
//! `docs/V2_1_PLAN.md` §2.6.
//!
//! - **typical per-tab < 100 MB** — the v2.1.0 mission gate; measured
//!   44.6 MB/tab on the product (10× example.com, shared offscreen
//!   context). The headless gate here asserts the same envelope on the
//!   engine path (measured ~28 MB/tab) so engine-side regressions are
//!   caught even when the CI job has no windowing stack for the shell.
//! - **heavy-page regression floor ≤ 600 MB** — cnn.com measures
//!   476–480 MB peak (brows-perf protocol); the floor guards against
//!   future spikes. The mission's aspirational < 200 MB heavy target is
//!   documented as NOT MET this cycle: the remainder is the page's own
//!   structural weight (JS heap ~95 MB incl. adtech iframes, layout box
//!   tree, non-instrumented display-list/scratch), which needs upstream
//!   engine work, not embedder knobs.

use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

fn perf_bin() -> PathBuf {
    std::env::var_os("BROWS12_GATE_BIN")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("target/release/brows-perf"))
}

/// Spawn brows-perf with the given URLs, wait for the JSON, return the
/// parsed report. The tree sampler lives in the heavy test; this one
/// relies on `run_perf`'s own idle RSS sample.
fn run_perf_with_idle(urls: &[&str], idle_secs: u64) -> serde_json::Value {
    let json = std::env::temp_dir().join("b12_gate_typical.json");
    let _ = std::fs::remove_file(&json);
    let mut cmd = Command::new(perf_bin());
    for u in urls {
        cmd.arg("--url").arg(u);
    }
    cmd.arg("--idle-secs")
        .arg(idle_secs.to_string())
        .arg("--json")
        .arg(&json)
        .stdout(std::process::Stdio::null());
    let status = cmd.status().expect("run brows-perf");
    assert!(status.success(), "brows-perf failed");
    let text = std::fs::read_to_string(&json).expect("perf json");
    let _ = std::fs::remove_file(&json);
    serde_json::from_str(&text).expect("parse perf json")
}

/// Typical-page gate: 10× example.com must stay under 100 MB per tab.
/// `run_perf` loads tabs sequentially and keeps every tab alive; the
/// idle RSS at the end of the window is the steady multi-tab floor.
#[test]
#[ignore = "e2e gate: needs release binary + GL + network; run in CI"]
fn gate_typical_per_tab_under_100mb() {
    let report = run_perf_with_idle(&["https://example.com/"; 10], 6);
    let rss_kb = report["idle"]["rss_kb"].as_u64().expect("idle rss_kb");
    let per_tab_mb = rss_kb as f64 / 10.0 / 1024.0;
    assert!(per_tab_mb < 100.0, "typical per-tab RSS {per_tab_mb:.1} MB exceeded the 100 MB gate");
}

/// Heavy-page regression floor: cnn.com peak (tree-sampled VmHWM of the
/// perf process) must stay ≤ 600 MB.
#[test]
#[ignore = "e2e gate: needs release binary + GL + network; run in CI"]
fn gate_heavy_page_regression_floor() {
    let json = std::env::temp_dir().join("b12_gate_cnn.json");
    let _ = std::fs::remove_file(&json);
    let mut child = Command::new(perf_bin())
        .args([
            "--url",
            "https://www.cnn.com/",
            "--settle-ms",
            "6000",
            "--timeout-ms",
            "90000",
            "--json",
        ])
        .arg(&json)
        .stdout(std::process::Stdio::null())
        .spawn()
        .expect("spawn brows-perf for cnn");

    let mut peak_kb = 0u64;
    let deadline = Instant::now() + Duration::from_secs(180);
    while Instant::now() < deadline {
        if matches!(child.try_wait(), Ok(Some(_))) {
            break;
        }
        if let Ok(status) = std::fs::read_to_string(format!("/proc/{}/status", child.id())) {
            for line in status.lines() {
                if let Some(rest) = line.strip_prefix("VmHWM:") {
                    if let Ok(kb) = rest.trim().trim_end_matches(" kB").parse::<u64>() {
                        peak_kb = peak_kb.max(kb);
                    }
                }
            }
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    let _ = child.kill();
    let _ = std::fs::remove_file(&json);
    let peak_mb = peak_kb as f64 / 1024.0;
    assert!(
        peak_mb > 0.0 && peak_mb <= 600.0,
        "cnn peak {peak_mb:.1} MB outside the (0, 600] regression floor"
    );
}
