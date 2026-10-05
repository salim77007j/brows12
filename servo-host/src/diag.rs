//! v2.1 Phase 1.1 — staged-load diagnostics for memory attribution.
//!
//! Setting `BROWS12_DIAG_DENY="images,scripts"` makes the resource-load
//! path serve an empty 200 for the listed request kinds *before* the
//! privacy verdict, so the peak-RSS delta between two runs attributes
//! memory to the denied subsystem's pipeline (image decode cache, JS
//! parse/heap, stylesheet parse, ...). Measurement tool only: it never
//! touches the privacy counters, it prints a one-time stderr notice when
//! active, and it is inert unless the env var is present.

use std::sync::OnceLock;

use brows12_privacy::blocker::RequestKind;

/// Which request kinds the diagnostic deny intercepts.
#[derive(Default, Clone, Copy, PartialEq, Eq, Debug)]
pub struct DiagDeny {
    pub images: bool,
    pub scripts: bool,
    pub stylesheets: bool,
    pub fonts: bool,
    pub media: bool,
    pub xhr: bool,
    pub frames: bool,
}

impl DiagDeny {
    /// Parse a comma-separated spec; unknown tokens are reported on
    /// stderr and ignored (mirrors `BROWS12_SET_PREF` behavior).
    pub fn parse(spec: &str) -> Self {
        let mut d = DiagDeny::default();
        for tok in spec.split(',') {
            match tok.trim().to_ascii_lowercase().as_str() {
                "images" => d.images = true,
                "scripts" => d.scripts = true,
                "stylesheets" | "css" => d.stylesheets = true,
                "fonts" => d.fonts = true,
                "media" => d.media = true,
                "xhr" => d.xhr = true,
                "frames" => d.frames = true,
                "" => {}
                other => eprintln!("[brows12-diag] unknown BROWS12_DIAG_DENY token: {other}"),
            }
        }
        d
    }

    pub fn any(&self) -> bool {
        self.images
            || self.scripts
            || self.stylesheets
            || self.fonts
            || self.media
            || self.xhr
            || self.frames
    }

    pub fn denies(&self, kind: RequestKind) -> bool {
        match kind {
            RequestKind::Image => self.images,
            RequestKind::Script => self.scripts,
            RequestKind::Stylesheet => self.stylesheets,
            RequestKind::Font => self.fonts,
            RequestKind::Media => self.media,
            RequestKind::Xhr => self.xhr,
            RequestKind::Subdocument => self.frames,
            RequestKind::Document | RequestKind::Other => false,
        }
    }

    pub fn active_names(&self) -> Vec<&'static str> {
        let mut names = Vec::new();
        if self.images {
            names.push("images");
        }
        if self.scripts {
            names.push("scripts");
        }
        if self.stylesheets {
            names.push("stylesheets");
        }
        if self.fonts {
            names.push("fonts");
        }
        if self.media {
            names.push("media");
        }
        if self.xhr {
            names.push("xhr");
        }
        if self.frames {
            names.push("frames");
        }
        names
    }
}

/// The parsed deny set for this process (empty when the env var is unset).
pub fn deny_set() -> DiagDeny {
    static SET: OnceLock<DiagDeny> = OnceLock::new();
    *SET.get_or_init(|| {
        let Ok(spec) = std::env::var("BROWS12_DIAG_DENY") else {
            return DiagDeny::default();
        };
        let set = DiagDeny::parse(&spec);
        if set.any() {
            eprintln!(
                "[brows12-diag] diagnostic deny ACTIVE for: {} \
                 (measurement tool — privacy counters unaffected)",
                set.active_names().join(",")
            );
        }
        set
    })
}

/// True when this request kind is intercepted by the diagnostic deny.
pub fn denied(kind: RequestKind) -> bool {
    deny_set().denies(kind)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_empty_is_inert() {
        let d = DiagDeny::parse("");
        assert!(!d.any());
        assert!(!d.denies(RequestKind::Image));
        assert!(!d.denies(RequestKind::Document));
    }

    #[test]
    fn parse_maps_tokens_to_kinds() {
        let d = DiagDeny::parse("images, scripts");
        assert!(d.any());
        assert!(d.denies(RequestKind::Image));
        assert!(d.denies(RequestKind::Script));
        assert!(!d.denies(RequestKind::Stylesheet));
        assert!(!d.denies(RequestKind::Document));
    }

    #[test]
    fn parse_accepts_css_alias_and_ignores_unknown() {
        let d = DiagDeny::parse("css, bogus-token");
        assert!(d.stylesheets);
        assert!(!d.any() || !d.scripts); // only stylesheets set
        assert_eq!(d.active_names(), vec!["stylesheets"]);
    }

    #[test]
    fn deny_set_without_env_is_inert() {
        // The test process does not set BROWS12_DIAG_DENY; guard against a
        // caller that did, by only asserting the env-independent part.
        if std::env::var("BROWS12_DIAG_DENY").is_err() {
            assert!(!deny_set().any());
            assert!(!denied(RequestKind::Image));
        }
    }
}
