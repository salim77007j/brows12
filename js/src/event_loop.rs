//! Event loop machinery: timers, task channel, cooperative interrupts.

use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Work completed off the JS thread, delivered to the event loop.
#[derive(Debug, Clone)]
pub enum JsTask {
    FetchDone {
        id: u32,
        err: Option<String>,
        status: u16,
        headers: Vec<(String, String)>,
        body: Vec<u8>,
    },
    /// WebSocket lifecycle/data event. `kind`: open | message | close | error.
    WsEvent { id: u32, kind: String, data: String },
    /// A message posted from a worker realm to its creating realm.
    WorkerMessage {
        id: u32,
        /// JSON-encoded message payload.
        data: String,
    },
    /// A worker failed to load or evaluate its script.
    WorkerError { id: u32, err: String },
}

/// Create the task channel for one realm.
pub fn channel() -> (Sender<JsTask>, Receiver<JsTask>) {
    std::sync::mpsc::channel()
}

static INTERRUPT: AtomicBool = AtomicBool::new(false);

/// Arm the interrupt: running scripts will abort at the next check.
pub fn request_interrupt() {
    INTERRUPT.store(true, Ordering::SeqCst);
}

/// Clear the interrupt flag (before each budgeted evaluation).
pub fn clear_interrupt() {
    INTERRUPT.store(false, Ordering::SeqCst);
}

/// Interrupt handler probe (used by the QuickJS interrupt hook).
pub fn interrupt_requested() -> bool {
    INTERRUPT.load(Ordering::SeqCst)
}

/// OS-timer queue for setTimeout/setInterval.
#[derive(Debug, Default)]
pub struct TimerQueue {
    heap: BinaryHeap<Reverse<TimerEntry>>,
    next_id: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TimerEntry {
    when: Instant,
    id: u64,
    #[allow(dead_code)]
    interval: bool,
}

impl Ord for TimerEntry {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.when.cmp(&other.when)
    }
}

impl PartialOrd for TimerEntry {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl TimerQueue {
    pub fn start(&mut self, delay: Duration, interval: bool) -> u64 {
        self.next_id += 1;
        let id = self.next_id;
        self.heap.push(Reverse(TimerEntry { when: Instant::now() + delay, id, interval }));
        id
    }

    pub fn cancel(&mut self, id: u64) {
        self.heap.retain(|Reverse(e)| e.id != id);
    }

    pub fn reschedule(&mut self, id: u64, delay: Duration) {
        self.heap.push(Reverse(TimerEntry { when: Instant::now() + delay, id, interval: true }));
    }

    /// Pop ids whose deadline passed.
    pub fn pop_due(&mut self, now: Instant) -> Vec<u64> {
        let mut due = Vec::new();
        while let Some(Reverse(entry)) = self.heap.peek() {
            if entry.when <= now {
                let Reverse(entry) = self.heap.pop().unwrap();
                due.push(entry.id);
            } else {
                break;
            }
        }
        due
    }

    pub fn next_deadline(&self) -> Option<Instant> {
        self.heap.peek().map(|Reverse(e)| e.when)
    }

    pub fn is_empty(&self) -> bool {
        self.heap.is_empty()
    }
}

/// The realm's task receiver, guarded for `&self` access.
pub struct TaskReceiver(pub Mutex<Receiver<JsTask>>);

impl TaskReceiver {
    pub fn recv_timeout(
        &self,
        timeout: Duration,
    ) -> std::result::Result<JsTask, std::sync::mpsc::RecvTimeoutError> {
        self.0.lock().expect("task receiver poisoned").recv_timeout(timeout)
    }
}
