//! Dedicated workers: one extra QuickJS realm per worker, pinned to its own
//! thread with its own memory limit and event loop.
//!
//! Messaging model (structured clone is JSON for this release):
//!
//! ```text
//! page realm                     worker thread
//! ──────────                     ─────────────
//! new Worker(url) ──fetch──▶ script source
//! workerPost(id, json) ────────▶ __brows12WorkerDeliver(json)
//! ◀──────── WorkerMessage{id} ── self.postMessage(data)
//! workerTerminate(id) ────────▶ flag + channel disconnect
//! ```

use crate::environment::JsEnvironment;
use crate::event_loop::JsTask;
use crate::runtime::JsRuntime;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::sync::Arc;
use std::time::Duration;

/// Handle to a running worker realm, kept in the creating realm's registry.
#[derive(Clone)]
pub struct WorkerHandle {
    tx: Sender<String>,
    pub(crate) terminate: Arc<AtomicBool>,
}

impl WorkerHandle {
    /// Deliver a JSON-encoded message into the worker realm. Safe to call
    /// before the worker has finished loading — messages are queued.
    pub fn post(&self, json: String) {
        let _ = self.tx.send(json);
    }

    /// Ask the worker thread to stop at its next loop check.
    pub fn terminate(&self) {
        self.terminate.store(true, Ordering::SeqCst);
    }
}

/// Pre-create a worker's mailbox. The [`WorkerHandle`] can be registered
/// immediately (so early `postMessage` calls queue instead of being lost);
/// the [`Receiver`] is handed to [`run_in_thread`] once the script is loaded.
pub fn mailbox() -> (WorkerHandle, Receiver<String>) {
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    (WorkerHandle { tx, terminate: Arc::new(AtomicBool::new(false)) }, rx)
}

/// Spawn a worker thread running `script` in a fresh realm.
///
/// `upstream` receives [`JsTask::WorkerMessage`] / [`JsTask::WorkerError`]
/// events destined for the creating realm's event loop.
pub fn spawn(
    id: u32,
    script: String,
    env: Arc<JsEnvironment>,
    upstream: Sender<JsTask>,
) -> WorkerHandle {
    let (handle, rx) = mailbox();
    let terminate = handle.terminate.clone();
    run_in_thread(id, script, env, upstream, rx, terminate);
    handle
}

/// Spawn the worker thread for a pre-created mailbox (see [`mailbox`]).
pub fn run_in_thread(
    id: u32,
    script: String,
    env: Arc<JsEnvironment>,
    upstream: Sender<JsTask>,
    rx: Receiver<String>,
    terminate: Arc<AtomicBool>,
) {
    let upstream_err = upstream.clone();

    let spawned = std::thread::Builder::new()
        .name(format!("brows12-worker-{id}"))
        .spawn(move || run_realm(id, script, env, rx, upstream, terminate));
    if spawned.is_err() {
        let _ = upstream_err
            .send(JsTask::WorkerError { id, err: "failed to spawn worker thread".into() });
    }
}

fn run_realm(
    id: u32,
    script: String,
    env: Arc<JsEnvironment>,
    rx: Receiver<String>,
    upstream: Sender<JsTask>,
    terminate: Arc<AtomicBool>,
) {
    let rt = match JsRuntime::new_worker(env, id, upstream.clone()) {
        Ok(rt) => rt,
        Err(e) => {
            let _ = upstream.send(JsTask::WorkerError { id, err: e.to_string() });
            return;
        }
    };
    if let Err(e) = rt.load_glue() {
        let _ = upstream.send(JsTask::WorkerError { id, err: e.to_string() });
        return;
    }
    tracing::debug!(target: "brows12::js::worker", "realm up, script {} bytes", script.len());
    if let Err(e) = rt.eval(&script) {
        let _ = upstream.send(JsTask::WorkerError { id, err: e.to_string() });
        return;
    }

    loop {
        if terminate.load(Ordering::SeqCst) {
            return;
        }
        match rx.try_recv() {
            Ok(json) => deliver(&rt, &json),
            Err(TryRecvError::Empty) => match rx.recv_timeout(Duration::from_millis(25)) {
                Ok(json) => deliver(&rt, &json),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            },
            Err(TryRecvError::Disconnected) => return,
        }
        // Pump timers, microtasks and completed fetches.
        let _ = rt.run_event_loop(Duration::from_millis(5));
    }
}

fn deliver(rt: &JsRuntime, json: &str) {
    tracing::debug!(target: "brows12::js::worker", "delivering: {}", json);
    let escaped = json.replace('\\', "\\\\").replace('\'', "\\'");
    if let Err(e) = rt.eval(&format!("__brows12WorkerDeliver('{escaped}');")) {
        tracing::warn!(target: "brows12::js::worker", "deliver eval error: {e}");
    }
}
