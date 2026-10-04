//! Phase 4.3.6 — memory-pressure (PSI) source. Reads Linux Pressure
//! Stall Information for memory from:
//! - `/proc/pressure/memory` (system-wide, always present on kernels
//!   with PSI enabled), and
//! - `/sys/fs/cgroup/memory.pressure` (cgroup v2, when the browser runs
//!   inside a cgroup with a memory controller — containers).
//!
//! PSI answers the question the RSS budget cannot: is the *system*
//! thrashing right now? A browser can be under its own budget while the
//! machine as a whole is swapping — and conversely, PSI lets the
//! governor release memory *before* our own budget is blown.
//!
//! File formats:
//! - /proc/pressure/memory (and cgroup v1):
//!   `some avg10=0.00 avg60=0.00 avg300=0.00 total=0`
//!   `full avg10=0.00 avg60=0.00 avg300=0.00 total=0`
//! - cgroup v2 memory.pressure: the same two lines, but `total=` in
//!   microseconds.
//!
//! Policy: `full` (all non-halt tasks stalled — real thrashing) drives
//! Critical; `some` (at least one task stalled) drives Elevated at a
//! higher threshold. Averages are used (avg10 primarily, avg60 to hold
//! the level through spikes) so a single stalled task for 10 ms cannot
//! trigger a hibernation storm.

use crate::memory::Pressure;
use std::time::Duration;

/// One PSI reading (percentages).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct PsiSnapshot {
    /// `%` of time at least one task stalled on memory.
    pub some_avg10: f32,
    /// `%` of time all non-halt tasks stalled (thrashing).
    pub full_avg10: f32,
    /// 60-second `full` average — holds the level through spikes.
    pub full_avg60: f32,
}

impl PsiSnapshot {
    /// Parse PSI text (either the /proc/pressure or cgroup v2 format).
    /// Returns None if neither a `some` nor a `full` line is found.
    pub fn parse(text: &str) -> Option<PsiSnapshot> {
        let mut snap = PsiSnapshot::default();
        let mut saw_any = false;
        for line in text.lines() {
            let (kind, rest) = match line.split_once(' ') {
                Some(("some", rest)) => ("some", rest),
                Some(("full", rest)) => ("full", rest),
                _ => continue,
            };
            let avg10 = rest
                .split_whitespace()
                .find_map(|kv| kv.strip_prefix("avg10="))
                .and_then(|v| v.parse::<f32>().ok());
            let Some(avg10) = avg10 else { continue };
            let avg60 = rest
                .split_whitespace()
                .find_map(|kv| kv.strip_prefix("avg60="))
                .and_then(|v| v.parse::<f32>().ok())
                .unwrap_or(0.0);
            match kind {
                "some" => {
                    snap.some_avg10 = avg10;
                    saw_any = true;
                }
                _ => {
                    snap.full_avg10 = avg10;
                    snap.full_avg60 = avg60;
                    saw_any = true;
                }
            }
        }
        if saw_any { Some(snap) } else { None }
    }
}

/// PSI thresholds. Defaults follow the kernel/ChromeOS convention:
/// `full` ≥ 5% (avg10) or ≥ 2% (avg60) is real thrashing; `some` ≥ 25%
/// (avg10) is sustained contention. Env-tunable for CI.
#[derive(Debug, Clone)]
pub struct PsiConfig {
    /// Master switch. Env: `BROWS12_PSI` ("0" disables).
    pub enabled: bool,
    /// `full avg10` above this → Critical. Env: `BROWS12_PSI_FULL10` (default 5.0).
    pub full10_critical: f32,
    /// `full avg60` above this → Critical (sustained). Env: `BROWS12_PSI_FULL60` (default 2.0).
    pub full60_critical: f32,
    /// `some avg10` above this → Elevated. Env: `BROWS12_PSI_SOME10` (default 25.0).
    pub some10_elevated: f32,
    /// Minimum interval between PSI-triggered reclaims, so a sustained
    /// spike reclaims at a humane pace instead of hibernating the whole
    /// strip in one tick. Env: `BROWS12_PSI_COOLDOWN_MS` (default 10_000).
    pub cooldown: Duration,
}

impl Default for PsiConfig {
    fn default() -> Self {
        PsiConfig {
            enabled: true,
            full10_critical: 5.0,
            full60_critical: 2.0,
            some10_elevated: 25.0,
            cooldown: Duration::from_millis(10_000),
        }
    }
}

impl PsiConfig {
    pub fn from_env() -> Self {
        let mut cfg = PsiConfig::default();
        if let Ok(v) = std::env::var("BROWS12_PSI") {
            cfg.enabled = v != "0";
        }
        if let Ok(v) = std::env::var("BROWS12_PSI_FULL10") {
            if let Ok(f) = v.parse::<f32>() {
                cfg.full10_critical = f;
            }
        }
        if let Ok(v) = std::env::var("BROWS12_PSI_FULL60") {
            if let Ok(f) = v.parse::<f32>() {
                cfg.full60_critical = f;
            }
        }
        if let Ok(v) = std::env::var("BROWS12_PSI_SOME10") {
            if let Ok(f) = v.parse::<f32>() {
                cfg.some10_elevated = f;
            }
        }
        if let Ok(v) = std::env::var("BROWS12_PSI_COOLDOWN_MS") {
            if let Ok(ms) = v.parse::<u64>() {
                cfg.cooldown = Duration::from_millis(ms.max(250));
            }
        }
        cfg
    }
}

/// System-wide PSI from `/proc/pressure/memory`.
pub fn read_system_psi() -> Option<PsiSnapshot> {
    let text = std::fs::read_to_string("/proc/pressure/memory").ok()?;
    PsiSnapshot::parse(&text)
}

/// cgroup v2 PSI of the current cgroup, when the file is exposed
/// (containers commonly mount it at /sys/fs/cgroup/memory.pressure).
pub fn read_cgroup_v2_psi() -> Option<PsiSnapshot> {
    let text = std::fs::read_to_string("/sys/fs/cgroup/memory.pressure").ok()?;
    PsiSnapshot::parse(&text)
}

/// Best available reading: cgroup v2 (the tighter scope) wins when
/// present, else system-wide.
pub fn read_psi() -> Option<(PsiSnapshot, &'static str)> {
    if let Some(s) = read_cgroup_v2_psi() {
        return Some((s, "cgroup-v2"));
    }
    read_system_psi().map(|s| (s, "system"))
}

/// Map a PSI reading to a pressure level (independent of our own RSS).
pub fn psi_level(snap: &PsiSnapshot, cfg: &PsiConfig) -> Pressure {
    if !cfg.enabled {
        return Pressure::Nominal;
    }
    if snap.full_avg10 >= cfg.full10_critical || snap.full_avg60 >= cfg.full60_critical {
        Pressure::Critical
    } else if snap.some_avg10 >= cfg.some10_elevated {
        Pressure::Elevated
    } else {
        Pressure::Nominal
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PROC_TEXT: &str = "some avg10=0.00 avg60=0.00 avg300=0.00 total=0\n\
                             full avg10=0.00 avg60=0.00 avg300=0.00 total=0\n";
    const THRASHING: &str = "some avg10=41.71 avg60=12.05 avg300=3.11 total=92814411\n\
                             full avg10=7.92 avg60=2.03 avg300=0.51 total=22534111\n";
    const CGROUP_V2_TEXT: &str = "some avg10=31.02 avg60=10.44 avg300=2.03 total=4402832\n\
                                  full avg10=0.00 avg60=0.00 avg300=0.00 total=0\n";

    #[test]
    fn parse_proc_format() {
        let snap = PsiSnapshot::parse(PROC_TEXT).expect("parses");
        assert_eq!(snap.some_avg10, 0.0);
        assert_eq!(snap.full_avg10, 0.0);
        let hot = PsiSnapshot::parse(THRASHING).expect("parses");
        assert!((hot.some_avg10 - 41.71).abs() < 1e-4);
        assert!((hot.full_avg10 - 7.92).abs() < 1e-4);
        assert!((hot.full_avg60 - 2.03).abs() < 1e-4);
    }

    #[test]
    fn parse_cgroup_v2_format() {
        let snap = PsiSnapshot::parse(CGROUP_V2_TEXT).expect("parses");
        assert!((snap.some_avg10 - 31.02).abs() < 1e-4);
        assert_eq!(snap.full_avg10, 0.0);
    }

    #[test]
    fn parse_garbage_is_none() {
        assert!(PsiSnapshot::parse("").is_none());
        assert!(PsiSnapshot::parse("hello world\nnot psi\n").is_none());
    }

    #[test]
    fn psi_levels_map() {
        let cfg = PsiConfig::default();
        let calm = PsiSnapshot::parse(PROC_TEXT).unwrap();
        assert_eq!(psi_level(&calm, &cfg), Pressure::Nominal);
        let thrash = PsiSnapshot::parse(THRASHING).unwrap();
        assert_eq!(psi_level(&thrash, &cfg), Pressure::Critical);
        let some_only = PsiSnapshot::parse(CGROUP_V2_TEXT).unwrap();
        assert_eq!(psi_level(&some_only, &cfg), Pressure::Elevated);
        // Disabled → always Nominal.
        let off = PsiConfig { enabled: false, ..cfg };
        assert_eq!(psi_level(&thrash, &off), Pressure::Nominal);
    }

    #[test]
    fn psi_env_overrides() {
        std::env::set_var("BROWS12_PSI_FULL10", "90");
        let cfg = PsiConfig::from_env();
        std::env::remove_var("BROWS12_PSI_FULL10");
        assert_eq!(cfg.full10_critical, 90.0);
    }
}
