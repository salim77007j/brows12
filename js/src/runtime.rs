//! The QuickJS-ng runtime wrapper: construction, limits, evaluation,
//! and the integrated event loop.

use crate::bindings;
use crate::bindings::RealmLinks;
use crate::dom_handle::DomHandle;
use crate::environment::JsEnvironment;
use crate::event_loop::{self, JsTask, TaskReceiver, TimerQueue};
use crate::glue::GLUE;
use rquickjs::{Context, Ctx, Runtime};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Hard cap for the JS heap (per realm) — runaway scripts trip the limit,
/// never the OOM killer.
pub const DEFAULT_MEMORY_LIMIT: usize = 256 * 1024 * 1024;
/// JS stack cap.
pub const DEFAULT_STACK_LIMIT: usize = 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum JsExecutionError {
    #[error("script threw: {0}")]
    Script(String),
    #[error("interrupted (budget exhausted or memory limit)")]
    Interrupted,
}

/// A page script to execute.
#[derive(Debug, Clone)]
pub enum Script {
    /// Inline `<script>` body.
    Inline { source: String, name: String },
    /// External script already fetched by the engine.
    External { content: String, name: String },
}

/// One QuickJS runtime + context. `!Send` by design — pin it to the page's
/// script thread.
pub struct JsRuntime {
    runtime: Runtime,
    ctx: Context,
    timers: Arc<Mutex<TimerQueue>>,
    receiver: Arc<TaskReceiver>,
    _keep_sender: std::sync::mpsc::Sender<JsTask>,
}

impl JsRuntime {
    /// Create a page realm with memory limits, an interrupt hook, native
    /// bindings and the glue layer.
    pub fn new(env: Arc<JsEnvironment>, dom: DomHandle) -> Result<Self, crate::JsError> {
        Self::build(env, dom, None, None)
    }

    /// Create a worker realm: no DOM bindings, messages flow upstream to the
    /// creating realm's event loop via [`JsTask::WorkerMessage`].
    pub fn new_worker(
        env: Arc<JsEnvironment>,
        worker_id: u32,
        upstream: Sender<JsTask>,
    ) -> Result<Self, crate::JsError> {
        let dom = DomHandle::new(Arc::new(Mutex::new(brows12_html::Document::new())));
        Self::build(env, dom, Some(worker_id), Some(upstream))
    }

    fn build(
        env: Arc<JsEnvironment>,
        dom: DomHandle,
        worker_id: Option<u32>,
        upstream: Option<Sender<JsTask>>,
    ) -> Result<Self, crate::JsError> {
        let runtime = Runtime::new().map_err(|e| crate::JsError::Setup(e.to_string()))?;
        runtime.set_memory_limit(DEFAULT_MEMORY_LIMIT);
        runtime.set_max_stack_size(DEFAULT_STACK_LIMIT);

        // Cooperative interruption: the engine can abort a runaway script
        // through `event_loop::request_interrupt()`; the GC itself runs
        // incrementally inside QuickJS and never via this hook.
        runtime.set_interrupt_handler(Some(Box::new(event_loop::interrupt_requested)));

        let ctx = Context::full(&runtime).map_err(|e| crate::JsError::Setup(e.to_string()))?;

        let (sender, receiver): (_, Receiver<JsTask>) = event_loop::channel();
        let timers = Arc::new(Mutex::new(TimerQueue::default()));

        {
            let sender = sender.clone();
            ctx.with(|ctx| {
                let links = RealmLinks {
                    sender,
                    upstream,
                    worker_id,
                    ws_senders: Arc::new(Mutex::new(std::collections::HashMap::new())),
                    workers: Arc::new(Mutex::new(std::collections::HashMap::new())),
                };
                bindings::install(&ctx, env, dom, links, timers.clone())
                    .map_err(|e| crate::JsError::Setup(e.to_string()))
            })?;
        }

        let rt = Self {
            runtime,
            ctx,
            timers,
            receiver: Arc::new(TaskReceiver(Mutex::new(receiver))),
            _keep_sender: sender,
        };

        Ok(rt)
    }

    /// Evaluate the embedded glue (classes, registries, console, fetch...).
    /// Called once by the engine after construction; kept separate so tests
    /// can bisect load failures precisely.
    pub fn load_glue(&self) -> Result<(), crate::JsError> {
        self.eval_raw(GLUE).map(|_| ()).map_err(|e| crate::JsError::Setup(format!("glue: {e}")))
    }

    /// Evaluate a source string; returns the final value rendered as a
    /// string (handy for tests and tooling).
    pub fn eval(&self, code: &str) -> Result<String, JsExecutionError> {
        self.eval_raw(code)
    }

    fn eval_raw(&self, code: &str) -> Result<String, JsExecutionError> {
        event_loop::clear_interrupt();
        self.ctx.with(|ctx: Ctx<'_>| -> Result<String, JsExecutionError> {
            match ctx.eval::<rquickjs::Value, _>(code) {
                Ok(value) => Ok(value_to_string(&ctx, value)),
                Err(e) => Err(to_script_error(&ctx, e)),
            }
        })
    }

    /// Execute a page script (inline source or engine-fetched external).
    pub fn execute(&self, script: &crate::Script) -> Result<(), JsExecutionError> {
        let (source, name) = match script {
            crate::Script::Inline { source, name } => (source.clone(), name.clone()),
            crate::Script::External { content, name } => (content.clone(), name.clone()),
        };
        self.eval_raw(&source).map(|_| ()).map_err(|e| match e {
            JsExecutionError::Script(msg) => JsExecutionError::Script(format!("{name}: {msg}")),
            other => other,
        })
    }

    /// Drain pending jobs (microtasks / promise continuations).
    pub fn drain_jobs(&self) {
        while self.runtime.is_job_pending() {
            if self.runtime.execute_pending_job().is_err() {
                break;
            }
        }
    }

    /// Run a GC pass. Only called from the event loop when idle, so
    /// collection never interrupts user-visible script execution.
    pub fn collect_garbage(&self) {
        self.runtime.run_gc();
    }

    /// Drive the realm until everything is idle: pending jobs, OS timers,
    /// completed fetches. Returns when no work remains or after
    /// `idle_timeout` without incoming work.
    pub fn run_event_loop(&self, idle_timeout: Duration) -> Result<(), crate::JsError> {
        loop {
            self.drain_jobs();

            // Fire due timers.
            let due: Vec<u64> = {
                let mut timers = self.timers.lock().unwrap();
                timers.pop_due(Instant::now())
            };
            for id in due {
                let _ = self.eval_raw(&format!("__brows12TimerFire({id});"));
                self.drain_jobs();
            }

            let next_deadline = self.timers.lock().unwrap().next_deadline();
            let result = match next_deadline {
                Some(deadline) => {
                    let wait = deadline.saturating_duration_since(Instant::now()).min(idle_timeout);
                    self.receiver.recv_timeout(wait)
                }
                None => self.receiver.recv_timeout(idle_timeout),
            };

            match result {
                Ok(task) => self.dispatch(task)?,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    if next_deadline.is_none() {
                        // Fully idle: cooperative GC and stop.
                        self.collect_garbage();
                        return Ok(());
                    }
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    if next_deadline.is_none() {
                        self.collect_garbage();
                        return Ok(());
                    }
                }
            }
        }
    }

    fn dispatch(&self, task: JsTask) -> Result<(), crate::JsError> {
        match task {
            JsTask::FetchDone { id, err, status, headers, body } => {
                let headers_json = serde_json::to_string(
                    &headers
                        .iter()
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect::<std::collections::BTreeMap<String, String>>(),
                )
                .unwrap_or_else(|_| "{}".into());
                let body_text = String::from_utf8_lossy(&body)
                    .replace('\\', "\\\\")
                    .replace('\n', "\\n")
                    .replace('\r', "\\r")
                    .replace('\'', "\\'");
                let js = match err {
                    Some(e) => format!(
                        "__brows12FetchDone({}, \"{}\", 0, \"{{}}\", \"\");",
                        id,
                        e.replace('\\', "\\\\").replace('"', "\\\"")
                    ),
                    None => format!(
                        "__brows12FetchDone({}, null, {}, '{}', '{}');",
                        id,
                        status,
                        headers_json.replace('\'', "\\'"),
                        body_text
                    ),
                };
                self.eval_raw(&js).map(|_| ()).map_err(|e| crate::JsError::Script(e.to_string()))
            }
            JsTask::WsEvent { id, kind, data } => {
                let data_json = serde_json::to_string(&data).unwrap_or_else(|_| "\"\"".into());
                let js = format!(
                    "__brows12WsEvent({}, '{}', '{}');",
                    id,
                    kind.replace('\'', "\\'"),
                    data_json.replace('\'', "\\'")
                );
                self.eval_raw(&js).map(|_| ()).map_err(|e| crate::JsError::Script(e.to_string()))
            }
            JsTask::WorkerMessage { id, data } => {
                // `data` is already a JSON string (the glue layer serializes
                // postMessage payloads); embed it directly, not re-encoded.
                let escaped = data.replace('\\', "\\\\").replace('\'', "\\'");
                let js = format!("__brows12WorkerMessage({}, '{}');", id, escaped);
                self.eval_raw(&js).map(|_| ()).map_err(|e| crate::JsError::Script(e.to_string()))
            }
            JsTask::WorkerError { id, err } => {
                let err_json = serde_json::to_string(&err).unwrap_or_else(|_| "\"\"".into());
                let js =
                    format!("__brows12WorkerError({}, '{}');", id, err_json.replace('\'', "\\'"));
                self.eval_raw(&js).map(|_| ()).map_err(|e| crate::JsError::Script(e.to_string()))
            }
        }
    }
}

fn to_script_error(ctx: &Ctx<'_>, e: rquickjs::Error) -> JsExecutionError {
    if e.is_exception() {
        let caught = ctx.catch();
        let msg = caught
            .as_string()
            .and_then(|s| s.to_string().ok())
            .or_else(|| {
                let obj = caught.as_object()?;
                let m: Option<rquickjs::String> = obj.get("message").ok();
                m.and_then(|s| s.to_string().ok())
            })
            .or_else(|| {
                let obj = caught.as_object()?;
                let n: Option<rquickjs::String> = obj.get("name").ok();
                n.and_then(|s| s.to_string().ok())
            })
            .unwrap_or_else(|| format!("{e:?}"));
        JsExecutionError::Script(msg)
    } else {
        // Unknown / interrupted variants surface as generic errors here.
        JsExecutionError::Script(format!("{e}"))
    }
}

fn value_to_string<'js>(ctx: &Ctx<'js>, value: rquickjs::Value<'js>) -> String {
    if value.is_undefined() || value.is_null() {
        return String::new();
    }
    if let Some(s) = value.as_string() {
        if let Ok(v) = s.to_string() {
            return v;
        }
    }
    if let Ok(Some(json)) = ctx.json_stringify(value) {
        if let Ok(s) = json.to_string() {
            return s;
        }
    }
    String::new()
}
