//! Resilient rendering-backend selection (v2.1 Phase 4, GPU support).
//!
//! The v2.1 Phase 4 Windows build crashed on an AMD integrated GPU:
//! surfman's WGL backend hard-`assert`ed when `wglDXOpenDeviceNV` returned
//! NULL (the `WGL_NV_DX_interop` bridge is unreliable on integrated GPUs,
//! VMs and RDP sessions). That panic took the whole browser down before
//! any fallback could run.
//!
//! This module is the answer, in three layers (see docs/GPU_SUPPORT.md):
//!
//! 1. **The right backend per platform.** Windows builds now use surfman's
//!    ANGLE/D3D11 backend (the servo crate's `no-wgl` feature) — the same
//!    architecture Chrome/Firefox/Edge ship: every GPU vendor (Intel
//!    integrated, Intel Arc, AMD integrated, AMD Radeon, every NVIDIA
//!    generation, virtual GPUs) is reached through plain DXGI enumeration,
//!    and when no usable GPU exists the D3D11 WARP rasterizer (pure CPU)
//!    is one adapter request away. Linux keeps the EGL/GLX hardware path
//!    with Mesa llvmpipe as the software lane.
//! 2. **A patched surfman** (`patched/surfman`): every init/present-path
//!    `assert!` is converted into a normal surfman `Error`, so backend
//!    attempts *return* instead of panicking.
//! 3. **This factory.** Every backend attempt is wrapped in
//!    `catch_unwind` (surfman is third-party code; a stray panic anywhere
//!    must degrade to an error, never abort the browser), attempts are
//!    logged with the precise failure reason, and the first success wins.
//!    If every lane fails the caller receives a clean `Err` with a
//!    user-actionable report — the process exits with a message, not a
//!    panic backtrace.
//!
//! Users can force the software lane with the `--software` CLI flag or the
//! `gfx.software-rendering` preference (`BROWS12_SET_PREF=
//! "gfx.software-rendering=true"`).

use std::panic::{self, AssertUnwindSafe};
use std::rc::Rc;
use std::sync::Arc;

use servo::{RenderingContext, SoftwareRenderingContext, WindowRenderingContext};
use winit::dpi::PhysicalSize;
use winit::raw_window_handle::{HasDisplayHandle, HasWindowHandle};
use winit::window::Window;

/// Which lanes the factory is allowed to try.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GfxPolicy {
    /// Hardware first, software fallback (the default).
    Auto,
    /// Software only — never touch a hardware/GPU lane.
    ForceSoftware,
}

impl GfxPolicy {
    /// Resolve the policy from a `--software` CLI flag and the
    /// `gfx.software-rendering` preference (flag OR pref wins).
    pub fn from_flags(software_flag: bool) -> Self {
        if software_flag || software_rendering_pref() {
            GfxPolicy::ForceSoftware
        } else {
            GfxPolicy::Auto
        }
    }
}

/// The `gfx.software-rendering` brows12 preference.
///
/// brows12 preferences that live in Servo's `Preferences` struct flow
/// through `prefs::apply_env_overrides`; this one is a brows12-side knob
/// (the engine has no such pref), so it is read from the same
/// `BROWS12_SET_PREF` channel before that mechanism would reject it as
/// unknown:
///
/// ```text
/// BROWS12_SET_PREF="gfx.software-rendering=true" brows12-ui
/// ```
pub fn software_rendering_pref() -> bool {
    let Ok(spec) = std::env::var("BROWS12_SET_PREF") else {
        return false;
    };
    for pair in spec.split(',') {
        let Some((key, value)) = pair.split_once('=') else {
            continue;
        };
        if key.trim() == "gfx.software-rendering" {
            return matches!(value.trim(), "true" | "1" | "yes" | "on");
        }
    }
    false
}

/// One failed backend attempt (kept for diagnostics and the fatal report).
#[derive(Clone, Debug)]
pub struct Attempt {
    /// Backend lane name, e.g. `angle-d3d11-hardware` / `sw-warp`.
    pub backend: &'static str,
    /// Whether this lane is a software/CPU lane.
    pub software: bool,
    /// Why it failed (surfman error, probe error, or a caught panic).
    pub error: String,
}

/// A successful backend selection.
#[derive(Debug)]
pub struct Selected<T> {
    pub context: T,
    pub backend: &'static str,
    /// Lanes that failed before this one succeeded (empty when first).
    pub skipped: Vec<Attempt>,
}

/// A backend candidate: name + software flag + probe closure.
type Candidate<T> = (&'static str, bool, Box<dyn FnOnce() -> Result<T, String>>);

/// The mock-testable core of the factory.
///
/// Runs candidates in order inside `catch_unwind`; the first success wins;
/// panics become `Attempt`s like any other failure; the returned `Err`
/// carries the full attempt log. **This function never panics outward** —
/// if a probe panics, the panic hook is silenced for the duration and the
/// payload is captured into the attempt log.
pub fn try_backends<T>(candidates: Vec<Candidate<T>>) -> Result<Selected<T>, Vec<Attempt>> {
    let mut attempts: Vec<Attempt> = Vec::new();
    for (name, software, probe) in candidates {
        match run_probe(probe) {
            Ok(context) => {
                log_attempt_success(name, &attempts);
                return Ok(Selected { context, backend: name, skipped: attempts });
            }
            Err(error) => {
                eprintln!("brows12 gfx: backend `{name}` unavailable: {error}");
                attempts.push(Attempt { backend: name, software, error });
            }
        }
    }
    Err(attempts)
}

/// Run one probe with panic capture. `Ok` = the probe succeeded, `Err` =
/// probe error or caught panic (message extracted from the payload).
fn run_probe<T>(probe: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    let previous_hook = panic::take_hook();
    panic::set_hook(Box::new(|_| {}));
    let result = panic::catch_unwind(AssertUnwindSafe(probe));
    panic::set_hook(previous_hook);
    match result {
        Ok(Ok(context)) => Ok(context),
        Ok(Err(error)) => Err(error),
        Err(payload) => Err(panic_message(&payload)),
    }
}

fn panic_message(payload: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(text) = payload.downcast_ref::<&str>() {
        format!("panic: {text}")
    } else if let Some(text) = payload.downcast_ref::<String>() {
        format!("panic: {text}")
    } else {
        "panic: (non-string payload — surfman/FFI assert or abort)".to_string()
    }
}

fn log_attempt_success(name: &str, skipped: &[Attempt]) {
    if skipped.is_empty() {
        eprintln!("brows12 gfx: rendering backend `{name}` OK");
    } else {
        eprintln!(
            "brows12 gfx: rendering backend `{name}` OK (after {} fallback{}: {})",
            skipped.len(),
            if skipped.len() == 1 { "" } else { "s" },
            skipped.iter().map(|a| a.backend).collect::<Vec<_>>().join(", ")
        );
    }
}

// ---------------------------------------------------------------------------
// Windows helper: never call into ANGLE before libEGL.dll is loadable.
//
// surfman's ANGLE backend resolves EGL entry points through
// `LoadLibraryA("libEGL.dll")`; if the DLL is missing, the function table is
// all-NULL and the first EGL call would be a call through a null pointer —
// an *uncatchable* access violation. Probing the library first turns that
// into a normal, catchable failure (and a clear diagnostic).
// ---------------------------------------------------------------------------

#[cfg(windows)]
pub fn libegl_available() -> Result<(), String> {
    use winapi::um::libloaderapi::{GetModuleHandleA, LoadLibraryA};
    use winapi::um::winnt::LPCSTR;
    unsafe {
        // Already mapped (imported via the mozangle import lib)?
        let loaded = GetModuleHandleA(b"libEGL.dll\0".as_ptr() as LPCSTR);
        if !loaded.is_null() {
            return Ok(());
        }
        let handle = LoadLibraryA(b"libEGL.dll\0".as_ptr() as LPCSTR);
        if handle.is_null() {
            return Err("libEGL.dll could not be loaded — it must sit next to the \
                        executable (it ships in the brows12 zip; do not delete it)"
                .to_string());
        }
        Ok(())
    }
}

#[cfg(not(windows))]
pub fn libegl_available() -> Result<(), String> {
    Ok(())
}

// ---------------------------------------------------------------------------
// Real factories
// ---------------------------------------------------------------------------

/// Hardware window-backed context (the UI's parent context).
///
/// On Windows this is surfman's ANGLE/D3D11 backend (`no-wgl` build): the
/// adapter comes from plain DXGI enumeration (any vendor), not from
/// vendor-specific selection APIs. On Linux it is the EGL/GLX backend.
/// The window is passed as an owned `Arc` so the probe can resolve the
/// raw handles itself (they are lifetime-bound to the window borrow).
pub fn create_window_parent_context(
    window: Arc<Window>,
    size: PhysicalSize<u32>,
) -> Result<Selected<Rc<WindowRenderingContext>>, Vec<Attempt>> {
    let name: &'static str =
        if cfg!(windows) { "angle-d3d11-hardware" } else { "egl-glx-hardware" };
    let candidates: Vec<Candidate<Rc<WindowRenderingContext>>> = vec![(
        name,
        false,
        Box::new(move || {
            libegl_available()?;
            let display_handle =
                window.display_handle().map_err(|e| format!("display handle: {e}"))?;
            let window_handle =
                window.window_handle().map_err(|e| format!("window handle: {e}"))?;
            WindowRenderingContext::new(display_handle, window_handle, size)
                .map(Rc::new)
                .map_err(|e| format!("{e:?}"))
        }),
    )];
    try_backends(candidates)
}

/// Software (CPU) rendering context.
///
/// Windows: ANGLE over D3D11 WARP — Microsoft's software rasterizer that
/// ships with Windows itself, so this lane works with *no* GPU at all.
/// Linux: surfman's surfaceless-EGL Mesa lane (llvmpipe).
pub fn create_software_context(
    size: PhysicalSize<u32>,
) -> Result<Selected<Rc<SoftwareRenderingContext>>, Vec<Attempt>> {
    let name: &'static str = if cfg!(windows) { "angle-d3d11-warp" } else { "mesa-llvmpipe" };
    let candidates: Vec<Candidate<Rc<SoftwareRenderingContext>>> = vec![(
        name,
        true,
        Box::new(move || {
            libegl_available()?;
            SoftwareRenderingContext::new(size).map(Rc::new).map_err(|e| format!("{e:?}"))
        }),
    )];
    try_backends(candidates)
}

/// Context for the headless harness: software first (works with no display
/// server at all), hardware window path as the fallback.
pub fn create_headless_context(
    window: Arc<Window>,
    size: PhysicalSize<u32>,
    policy: GfxPolicy,
) -> Result<Selected<Rc<dyn RenderingContext>>, Vec<Attempt>> {
    let mut candidates: Vec<Candidate<Rc<dyn RenderingContext>>> = Vec::new();
    if policy == GfxPolicy::Auto {
        candidates.push((
            if cfg!(windows) { "angle-d3d11-hardware" } else { "egl-glx-hardware" },
            false,
            Box::new(move || {
                libegl_available()?;
                let display_handle =
                    window.display_handle().map_err(|e| format!("display handle: {e}"))?;
                let window_handle =
                    window.window_handle().map_err(|e| format!("window handle: {e}"))?;
                WindowRenderingContext::new(display_handle, window_handle, size)
                    .map(|ctx| Rc::new(ctx) as Rc<dyn RenderingContext>)
                    .map_err(|e| format!("{e:?}"))
            }),
        ));
    }
    candidates.push((
        if cfg!(windows) { "angle-d3d11-warp" } else { "mesa-llvmpipe" },
        true,
        Box::new(move || {
            libegl_available()?;
            SoftwareRenderingContext::new(size)
                .map(|ctx| Rc::new(ctx) as Rc<dyn RenderingContext>)
                .map_err(|e| format!("{e:?}"))
        }),
    ));
    try_backends(candidates)
}

/// The fatal report when no rendering backend works at all. Caller exits
/// the process cleanly with this — never a panic.
pub fn fatal_report(attempts: &[Attempt], hint: &str) -> String {
    let mut out = String::new();
    out.push_str("brows12: no usable rendering backend on this machine.\n");
    out.push_str("  Every lane of the fallback chain failed:\n");
    for a in attempts {
        out.push_str(&format!(
            "    - {} ({}): {}\n",
            a.backend,
            if a.software { "software" } else { "hardware" },
            a.error
        ));
    }
    out.push_str(&format!("  {hint}\n"));
    out.push_str("  Details: docs/GPU_SUPPORT.md in the brows12 repository.\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    /// Serialize tests that mutate the process-global env.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn ok<T>(v: T) -> Result<T, String> {
        Ok(v)
    }

    fn candidate<T>(
        name: &'static str,
        probe: impl FnOnce() -> Result<T, String> + 'static,
    ) -> Candidate<T> {
        (name, false, Box::new(probe))
    }

    #[test]
    fn first_success_wins_and_skips_nothing() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let calls2 = calls.clone();
        let cands: Vec<Candidate<u32>> = vec![
            candidate("a", move || {
                calls2.lock().unwrap().push("a");
                ok(1)
            }),
            candidate("b", || Err("unused".into())),
        ];
        let sel = try_backends(cands).expect("must select");
        assert_eq!(sel.backend, "a");
        assert_eq!(sel.context, 1);
        assert!(sel.skipped.is_empty());
        assert_eq!(*calls.lock().unwrap(), vec!["a"]);
    }

    #[test]
    fn error_falls_through_to_next_backend() {
        let cands: Vec<Candidate<&'static str>> = vec![
            candidate("broken", || Err("DeviceOpenFailed".into())),
            candidate("works", || ok("ctx")),
        ];
        let sel = try_backends(cands).expect("must fall through");
        assert_eq!(sel.backend, "works");
        assert_eq!(sel.skipped.len(), 1);
        assert_eq!(sel.skipped[0].backend, "broken");
        assert_eq!(sel.skipped[0].error, "DeviceOpenFailed");
    }

    #[test]
    fn probe_panic_is_caught_and_becomes_an_attempt() {
        let cands: Vec<Candidate<&'static str>> = vec![
            candidate("panicking", || {
                // Mirrors what the un-patched surfman wgl backend did.
                panic!("assertion failed: !gl_dx_interop_device.is_null()");
            }),
            candidate("rescue", || ok("software")),
        ];
        let sel = try_backends(cands).expect("panic must degrade to fallback");
        assert_eq!(sel.backend, "rescue");
        assert_eq!(sel.skipped.len(), 1);
        assert_eq!(sel.skipped[0].backend, "panicking");
        assert!(
            sel.skipped[0].error.contains("assertion failed"),
            "panic payload must be captured: {}",
            sel.skipped[0].error
        );
    }

    #[test]
    fn all_backends_failing_is_a_clean_err_never_a_panic() {
        let cands: Vec<Candidate<u8>> = vec![
            candidate("hw", || Err("NoAdapterFound".into())),
            candidate("sw", || Err("libEGL missing".into())),
        ];
        let attempts = try_backends(cands).expect_err("must report all attempts");
        assert_eq!(attempts.len(), 2);
        assert_eq!(attempts[0].backend, "hw");
        assert_eq!(attempts[1].backend, "sw");
        // The fatal report must be user-actionable.
        let report = fatal_report(&attempts, "try --software");
        assert!(report.contains("no usable rendering backend"));
        assert!(report.contains("--software"));
        assert!(report.contains("docs/GPU_SUPPORT.md"));
    }

    #[test]
    fn empty_candidate_list_is_a_clean_err() {
        let attempts = try_backends::<u8>(Vec::new()).expect_err("empty => err");
        assert!(attempts.is_empty());
    }

    #[test]
    fn software_flag_forces_force_software_policy() {
        assert_eq!(GfxPolicy::from_flags(true), GfxPolicy::ForceSoftware);
    }

    #[test]
    fn gfx_software_rendering_pref_is_parsed_from_env() {
        let _guard = ENV_LOCK.lock().unwrap();
        // unset
        std::env::remove_var("BROWS12_SET_PREF");
        assert!(!software_rendering_pref());
        // explicit true
        std::env::set_var("BROWS12_SET_PREF", "gfx.software-rendering=true");
        assert!(software_rendering_pref());
        // explicit false
        std::env::set_var("BROWS12_SET_PREF", "gfx.software-rendering=false");
        assert!(!software_rendering_pref());
        // mixed with other prefs (the documented form)
        std::env::set_var("BROWS12_SET_PREF", "js_mem_max=128,gfx.software-rendering=true");
        assert!(software_rendering_pref());
        // other keys only
        std::env::set_var("BROWS12_SET_PREF", "js_mem_max=128");
        assert!(!software_rendering_pref());
        std::env::remove_var("BROWS12_SET_PREF");
    }
}
