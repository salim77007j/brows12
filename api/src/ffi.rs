//! C ABI for non-Rust UI layers (feature `capi`).
//!
//! A minimal, stable surface: create an engine, open tabs, load URLs, and
//! pull frames + events. Configuration flows in as JSON strings so the ABI
//! never changes when config fields grow. See `api/include/brows12.h`.

use crate::Browser;

/// Opaque engine handle.
pub struct B12Engine(pub Browser);
/// Opaque tab handle.
pub struct B12Tab(pub std::sync::Arc<crate::Tab>);

#[no_mangle]
pub extern "C" fn b12_engine_new(config_json: *const std::os::raw::c_char) -> *mut B12Engine {
    let config_json = unsafe { cstr(config_json) };
    let mut builder = crate::Browser::builder();
    if let Ok(cfg) = serde_json::from_str::<serde_json::Value>(&config_json) {
        if let Some((w, h)) = cfg.get("viewport").and_then(|v| v.as_array()).and_then(|a| {
            Some((
                a.first()?.as_u64()? as u32,
                a.get(1)?.as_u64()? as u32,
            ))
        }) {
            builder = builder.viewport(w, h);
        }
        if let Some(ua) = cfg.get("user_agent").and_then(|v| v.as_str()) {
            builder = builder.user_agent(ua);
        }
        if let Some(max) = cfg.get("max_live_pages").and_then(|v| v.as_u64()) {
            builder = builder.max_live_pages(max as usize);
        }
    }
    Box::into_raw(Box::new(B12Engine(builder.build())))
}

/// # Safety
/// `engine` must have been returned by `b12_engine_new`.
#[no_mangle]
pub unsafe extern "C" fn b12_engine_free(engine: *mut B12Engine) {
    if !engine.is_null() {
        drop(Box::from_raw(engine));
    }
}

/// # Safety
/// `engine` must be valid.
#[no_mangle]
pub unsafe extern "C" fn b12_tab_new(engine: *mut B12Engine) -> *mut B12Tab {
    let engine = match engine.as_ref() {
        Some(e) => e,
        None => return std::ptr::null_mut(),
    };
    Box::into_raw(Box::new(B12Tab(std::sync::Arc::new(engine.0.new_tab()))))
}

/// # Safety
/// `tab` must have been returned by `b12_tab_new`.
#[no_mangle]
pub unsafe extern "C" fn b12_tab_free(tab: *mut B12Tab) {
    if !tab.is_null() {
        drop(Box::from_raw(tab));
    }
}

/// Load a URL; returns 0 on success, non-zero error code otherwise.
/// # Safety
/// `tab` and `url` must be valid.
#[no_mangle]
pub unsafe extern "C" fn b12_tab_load(tab: *mut B12Tab, url: *const std::os::raw::c_char) -> i32 {
    let tab = match tab.as_ref() {
        Some(t) => t,
        None => return 1,
    };
    let url = cstr(url);
    match tab.load_url(&url) {
        Ok(()) => 0,
        Err(_) => 2,
    }
}

/// Frame metadata. Returns 0 when a frame is available.
/// # Safety
/// `tab`, `w`, `h`, `data` must be valid pointers.
#[no_mangle]
pub unsafe extern "C" fn b12_tab_frame(
    tab: *const B12Tab,
    w: *mut u32,
    h: *mut u32,
    data: *mut *const u8,
    len: *mut usize,
) -> i32 {
    let tab = match tab.as_ref() {
        Some(t) => t,
        None => return 1,
    };
    match tab.frame() {
        Some(frame) => {
            if !w.is_null() {
                *w = frame.width;
            }
            if !h.is_null() {
                *h = frame.height;
            }
            if !data.is_null() {
                *data = frame.rgba_premultiplied().as_ptr();
            }
            if !len.is_null() {
                *len = frame.rgba_premultiplied().len();
            }
            0
        }
        None => 2,
    }
}

/// Title, copied into `buf` (NUL-terminated). Returns the needed length.
/// # Safety
/// `buf` may be null to query the required size.
#[no_mangle]
pub unsafe extern "C" fn b12_tab_title(tab: *const B12Tab, buf: *mut u8, cap: usize) -> usize {
    let Some(tab) = tab.as_ref() else { return 0 };
    let title = tab.title();
    let bytes = title.as_bytes();
    if !buf.is_null() && cap > bytes.len() {
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), buf, bytes.len());
        *buf.add(bytes.len()) = 0;
    }
    bytes.len() + 1
}

unsafe fn cstr<'a>(p: *const std::os::raw::c_char) -> std::borrow::Cow<'a, str> {
    if p.is_null() {
        return std::borrow::Cow::Borrowed("");
    }
    std::ffi::CStr::from_ptr(p).to_string_lossy()
}
