//! # brows12-js
//!
//! QuickJS-ng integration for the Brows12 engine, through `rquickjs`.
//!
//! Design notes (the interesting parts):
//!
//! * **No JS values are stored on the Rust side.** All callbacks (promise
//!   resolvers, timer callbacks, event listeners) live in JS registries
//!   (`__brows12Pending`, `__brows12Timers`, `__brows12Listeners`). Rust only
//!   exchanges plain data (ids, strings, numbers) across the boundary — this
//!   removes an entire class of GC-lifetime bugs.
//! * **Event loop**: a dedicated thread runs the QuickJS runtime; the loop
//!   interleaves pending jobs (microtasks/promises), OS timers and completed
//!   network fetches arriving over a channel. When everything is idle the
//!   runtime GC runs *cooperatively* — collections never interrupt script
//!   execution, keeping main-thread stalls bounded.
//! * **Memory limits**: the runtime gets a hard allocation limit and a stack
//!   cap at construction; runaway scripts trip an interrupt handler instead
//!   of the process.

pub mod bindings;
pub mod canvas;
pub mod dom_handle;
pub mod environment;
pub mod event_loop;
pub mod glsl;
pub mod glue;
pub mod glue_b64;
pub mod idb_bindings;
pub mod modules;
pub mod platform;
pub mod runtime;
pub mod wasm;
pub mod webgl;
pub mod webgl_bindings;
pub mod webgpu;
pub mod webgpu_bindings;
pub mod worker;

pub use bindings::{WorkerRegistry, WsSenders};
pub use canvas::{CanvasStore, CanvasSurface};
pub use dom_handle::DomHandle;
pub use environment::JsEnvironment;
pub use event_loop::JsTask;
pub use runtime::Script;
pub use runtime::{JsExecutionError, JsRuntime};
pub use webgl::{gpu_available, WebGlStore, WebGlSurface};
pub use worker::WorkerHandle;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum JsError {
    #[error("runtime setup failed: {0}")]
    Setup(String),
    #[error("script error: {0}")]
    Script(String),
}
