//! v2.1 Phase 1.2 — embedder completion criterion tests.
//!
//! The engine's `LoadStatus::Complete` (document `load` event) is blocked
//! indefinitely by hung third-party ad iframes (measured on MDN: 45 s
//! window, 5,485 frames rendered, never complete). The embedder criterion
//! (`HostState::completion`) accepts a quiet period or a hard cap as
//! fallbacks and keeps the raw flag separable.

use std::sync::atomic::Ordering;
use std::time::{Duration, SystemTime};

use servo_host::delegate::HostState;
use servo::LoadStatus;

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

#[test]
fn no_activity_is_incomplete() {
    let s = HostState::new();
    let (embedder, raw, criterion) = s.completion(2_000, 15_000);
    assert!(!embedder);
    assert!(!raw);
    assert_eq!(criterion, "no-activity-yet");
}

#[test]
fn engine_event_wins_and_reports_raw() {
    let s = HostState::new();
    *s.load_status.lock().unwrap() = Some(LoadStatus::Complete);
    let (embedder, raw, criterion) = s.completion(2_000, 15_000);
    assert!(embedder && raw);
    assert_eq!(criterion, "engine-load-event");
}

#[test]
fn quiet_period_completes_without_engine_event() {
    let s = HostState::new();
    let now = unix_ms();
    // First activity 10 s ago, last activity 5 s ago: well past a 2 s
    // quiet window, well inside a 15 s cap.
    s.first_activity_ms.store(now - 10_000, Ordering::Relaxed);
    s.last_activity_ms.store(now - 5_000, Ordering::Relaxed);
    let (embedder, raw, criterion) = s.completion(2_000, 15_000);
    assert!(embedder);
    assert!(!raw, "engine never fired: the raw flag stays honest");
    assert_eq!(criterion, "quiet-period");
}

#[test]
fn hard_cap_completes_polling_pages() {
    let s = HostState::new();
    let now = unix_ms();
    // Forever-polling page: activity every instant since first activity
    // 20 s ago; the 15 s cap must fire even though it is not quiet.
    s.first_activity_ms.store(now - 20_000, Ordering::Relaxed);
    s.last_activity_ms.store(now, Ordering::Relaxed);
    let (embedder, raw, criterion) = s.completion(2_000, 15_000);
    assert!(embedder);
    assert!(!raw);
    assert_eq!(criterion, "first-activity-cap");
}

#[test]
fn pending_while_activity_is_fresh() {
    let s = HostState::new();
    let now = unix_ms();
    s.first_activity_ms.store(now - 3_000, Ordering::Relaxed);
    s.last_activity_ms.store(now, Ordering::Relaxed);
    let (embedder, raw, criterion) = s.completion(2_000, 15_000);
    assert!(!embedder && !raw);
    assert_eq!(criterion, "pending");
}

#[test]
fn begin_navigation_resets_all_anchors() {
    let s = HostState::new();
    let now = unix_ms();
    s.first_activity_ms.store(now - 20_000, Ordering::Relaxed);
    s.last_activity_ms.store(now, Ordering::Relaxed);
    *s.embedder_complete_at.lock().unwrap() = Some(std::time::Instant::now());
    *s.complete_at.lock().unwrap() = Some(std::time::Instant::now());
    s.begin_navigation();
    assert_eq!(s.first_activity_ms.load(Ordering::Relaxed), 0);
    assert_eq!(s.last_activity_ms.load(Ordering::Relaxed), 0);
    assert!(s.embedder_complete_at.lock().unwrap().is_none());
    assert!(s.complete_at.lock().unwrap().is_none());
    let (embedder, _, _) = s.completion(2_000, 15_000);
    assert!(!embedder);
}

#[test]
fn settled_after_completion_latches_once_and_waits() {
    let s = HostState::new();
    let now = unix_ms();
    s.first_activity_ms.store(now - 10_000, Ordering::Relaxed);
    s.last_activity_ms.store(now - 5_000, Ordering::Relaxed);
    // Quiet-period page: completion is immediate, but a 400 ms settle
    // window must not be satisfied on the first poll.
    assert!(!s.settled_after_completion(2_000, 15_000, 400));
    std::thread::sleep(Duration::from_millis(450));
    assert!(s.settled_after_completion(2_000, 15_000, 400));
    // The latch anchors at first observation: subsequent polls stay true.
    assert!(s.settled_after_completion(2_000, 15_000, 400));
}

#[test]
fn env_helpers_have_sane_defaults() {
    if std::env::var("BROWS12_COMPLETE_QUIET_MS").is_err() {
        assert_eq!(servo_host::delegate::env_complete_quiet_ms(), 2_000);
    }
    if std::env::var("BROWS12_COMPLETE_MAX_WAIT_MS").is_err() {
        assert_eq!(servo_host::delegate::env_complete_max_wait_ms(), 15_000);
    }
}
