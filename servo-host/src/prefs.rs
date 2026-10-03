//! brows12 preferences for real-world sites AND the Phase 2 memory /
//! performance profile.
//!
//! Two layers:
//! 1. `compat_preferences` — Servo's defaults are conservative
//!    (prototype-first); real 2026 sites need the modern-DOM knobs on.
//!    Each flip is documented with the sites that motivated it.
//! 2. `brows12_preferences` — compat + the memory/performance knobs
//!    (Phase 2). Every knob records its rationale; the final report
//!    marks these as brows12 policy (embedder preferences, not engine
//!    patches).

use servo::prefs::Preferences;

/// The brows12 "real-world sites" profile (Phase 1). Applied by the
/// shell and the headless harness; the UI settings page can later
/// override per install.
pub fn compat_preferences() -> Preferences {
    // GitHub's front end calls `crypto.subtle` at boot; the `webcrypto`
    // cargo feature must also be enabled for the implementation to exist.
    // IntersectionObserver: GitHub, YouTube, reddit lazy-load and
    // virtualize through it.
    // Constructable stylesheets (`CSSStyleSheet` + adoptedStyleSheets):
    // GitHub's design system and many web components depend on it.
    // CSS Font Loading API (document.fonts): used by GitHub and MDN.
    // ResizeObserver is on by default upstream; keep explicit for clarity.
    Preferences {
        dom_crypto_subtle_enabled: true,
        dom_intersection_observer_enabled: true,
        dom_adoptedstylesheet_enabled: true,
        dom_fontface_enabled: true,
        dom_resize_observer_enabled: true,
        ..Preferences::default()
    }
}

/// The full brows12 profile: compatibility (above) + the Phase 2 memory
/// and performance knobs. This is what the shell and the headless
/// harness run by default.
pub fn brows12_preferences() -> Preferences {
    Preferences {
        // ---- Network cache (Phase 2.1: bound in-memory cache) --------
        // Servo default: 5000 entries in the in-memory HTTP cache
        // (evictions spill to the disk cache, nothing is lost — see
        // servo-net http_cache.rs `on_evict`). 1024 entries keeps the
        // hot working set of a normal browsing session while cutting
        // the resident cost of cache-after-a-long-session; a cache hit
        // miss just re-reads from disk.
        network_http_cache_size: 1024,

        // ---- JavaScript heap (Phase 2.1) -----------------------------
        // SpiderMonkey GC heap ceiling for the whole engine. Servo
        // default: -1 (unlimited). 256 MiB bounds runaway pages; the
        // per-tab saver is hibernation (the governor), this is the
        // second line of defense for a single page allocating hard.
        js_mem_max: 256 * 1024 * 1024,

        // ---- Timers (Phase 2.3: background throttling support) ------
        // Clamp for nested/repeated setTimeout: Servo default is 1000ms.
        // Kept explicit because the Phase 2.3 report leans on it: with
        // throttled webviews, engine timers in background tabs run at
        // this minimum duration on top of `set_throttled`'s own clamp.
        js_timers_minimum_duration: 1000,

        ..compat_preferences()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_layers_compat() {
        let compat = compat_preferences();
        let full = brows12_preferences();
        assert!(full.dom_intersection_observer_enabled);
        assert!(full.dom_adoptedstylesheet_enabled);
        assert_eq!(full.network_http_cache_size, 1024);
        assert_eq!(full.js_mem_max, 256 * 1024 * 1024);
        // Defaults we consciously keep:
        assert_eq!(full.js_timers_minimum_duration, 1000);
        // Guard against accidental regressions of the Servo defaults we
        // did NOT intend to touch:
        assert_eq!(
            full.network_http_disk_cache_size,
            compat.network_http_disk_cache_size
        );
    }
}
