//! brows12 compatibility preferences for real-world sites.
//!
//! Servo's defaults are conservative (prototype-first); real 2026 sites
//! need the modern-DOM knobs on. Each flip is documented with the sites
//! that motivated it, and the final report marks these as brows12 policy
//! (they are embedder preferences, not engine patches).

use servo::prefs::Preferences;

/// The brows12 "real-world sites" profile. Applied by the shell and the
/// headless harness; the UI settings page can later override per install.
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
