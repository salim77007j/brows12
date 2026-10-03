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
    let mut prefs = Preferences::default();

    // GitHub's front end calls `crypto.subtle` at boot; the `webcrypto`
    // cargo feature must also be enabled for the implementation to exist.
    prefs.dom_crypto_subtle_enabled = true;

    // IntersectionObserver: GitHub, YouTube, reddit lazy-load and
    // virtualize through it.
    prefs.dom_intersection_observer_enabled = true;

    // Constructable stylesheets (`CSSStyleSheet` + adoptedStyleSheets):
    // GitHub's design system and many web components depend on it.
    prefs.dom_adoptedstylesheet_enabled = true;

    // CSS Font Loading API (document.fonts): used by GitHub and MDN.
    prefs.dom_fontface_enabled = true;

    // ResizeObserver is on by default upstream; keep explicit for clarity.
    prefs.dom_resize_observer_enabled = true;

    prefs
}
