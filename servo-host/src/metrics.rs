//! Process-level performance metrics for the memory governor and the
//! Phase 2 benchmarks. Everything reads from `/proc` (Linux), so the
//! numbers are ground truth for this process — no engine guessing.
//!
//! Measurement rules (docs/SERVO_INTEGRATION_PLAN.md §5):
//! - RAM: `VmRSS` from `/proc/self/status` (resident, includes shared
//!   engine state) plus Servo's own `create_memory_report` for a
//!   per-subsystem breakdown.
//! - CPU: `(utime + stime)` from `/proc/self/stat`, the same counter
//!   `time(1)` and `perf` use.

use std::time::Instant;

/// Resident set size of this process, in KiB.
pub fn rss_kb() -> Option<u64> {
    field_from_status("VmRSS:")
}

/// Peak resident set size of this process, in KiB (VmHWM).
pub fn peak_rss_kb() -> Option<u64> {
    field_from_status("VmHWM:")
}

fn field_from_status(field: &str) -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix(field) {
            // "VmRSS:    123456 kB"
            return rest.trim().split_whitespace().next()?.parse().ok();
        }
    }
    None
}

/// Total CPU seconds consumed by this process since start (user + system).
pub fn cpu_seconds() -> Option<f64> {
    let stat = std::fs::read_to_string("/proc/self/stat").ok()?;
    // Fields after the second '(' may contain spaces; stat puts the command
    // in parens, so split from the right — utime is field 14, stime 15
    // (1-based, after the comm field). Safer: rsplit and index from the end.
    let close = stat.rfind(')')?;
    let fields = stat[close + 2..].split_whitespace().collect::<Vec<_>>();
    // After the comm field: state is fields[0], so utime is fields[11],
    // stime fields[12] (proc(5) positions 14/15 with pid+comm at 1/2).
    let utime: f64 = fields.get(11)?.parse().ok()?;
    let stime: f64 = fields.get(12)?.parse().ok()?;
    let hz = os_sysconf_clk_tck() as f64;
    Some((utime + stime) / hz)
}

fn os_sysconf_clk_tck() -> u64 {
    // No libc dependency needed: Linux userland ABI fixes CLK_TCK at 100
    // for /proc/stat purposes (ACKNOWLEDGED: utime/stime are in clock ticks
    // scaled by the kernel's USER_HZ, which is 100 on every shipped arch).
    100
}

/// Wall-clock anchored CPU meter. `percent()` returns the average CPU
/// utilization of this process between `start()` and the call — the number
/// the Phase 2 gates are written against (idle CPU < 1% with 10 tabs).
pub struct CpuMeter {
    t0: Instant,
    c0: Option<f64>,
}

impl CpuMeter {
    /// Anchor a measurement window now.
    pub fn start() -> Self {
        CpuMeter { t0: Instant::now(), c0: cpu_seconds() }
    }

    /// Average CPU% (0-100 × core count) over the window so far.
    /// More than 100 means more than one core busy on average.
    pub fn percent(&self) -> Option<f64> {
        let c1 = cpu_seconds()?;
        let c0 = self.c0?;
        let dt = self.t0.elapsed().as_secs_f64();
        if dt <= 0.0 {
            return None;
        }
        Some(100.0 * (c1 - c0) / dt)
    }

    /// Seconds elapsed in the window.
    pub fn elapsed_secs(&self) -> f64 {
        self.t0.elapsed().as_secs_f64()
    }
}

/// Quick snapshot for logs/JSON reports.
pub fn snapshot() -> MetricsSnapshot {
    MetricsSnapshot {
        rss_kb: rss_kb(),
        peak_rss_kb: peak_rss_kb(),
        cpu_seconds: cpu_seconds(),
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct MetricsSnapshot {
    pub rss_kb: Option<u64>,
    pub peak_rss_kb: Option<u64>,
    pub cpu_seconds: Option<f64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proc_files_exist_and_parse() {
        let rss = rss_kb().expect("VmRSS must parse on Linux");
        assert!(rss > 0);
        let peak = peak_rss_kb().expect("VmHWM must parse");
        assert!(peak >= rss);
        let cpu = cpu_seconds().expect("utime+stime must parse");
        assert!(cpu >= 0.0);
        let meter = CpuMeter::start();
        std::thread::sleep(std::time::Duration::from_millis(20));
        let pct = meter.percent().expect("meter");
        assert!(pct >= 0.0);
    }
}
