//! Response-header security guard — Phase 4 Focus Area 2.7.
//!
//! Servo 0.6 ships no engine-side enforcement for the classic webappsec
//! response headers (verified empirically: the net/script crates never
//! look at `Content-Security-Policy`, `X-Frame-Options`,
//! `Strict-Transport-Security`, COOP/COEP/CORP). The embedder cannot see
//! pass-through response headers either, so this module closes the gap
//! the only reliable embedder-side way: a header **probe**.
//!
//! For every top-level document (and cross-site frame target) brows12
//! fetches the response headers itself (small cached GET), parses the
//! security headers, and then enforces what is enforceable from the
//! request side at `load_web_resource`:
//!
//! * `Content-Security-Policy` → per-site policy (the Servo-team
//!   `content_security_policy` crate does the spec-accurate matching);
//!   every later subresource from that document is checked with
//!   `should_request_be_blocked` before Servo fetches it.
//! * `X-Frame-Options` + CSP `frame-ancestors` → cross-origin framing of
//!   the site is denied (clickjacking defense; same-origin frames pass).
//! * `Strict-Transport-Security` → learned into the HTTPS-Only upgrader's
//!   HSTS cache (runtime HSTS learning without an engine hook — the gap
//!   recorded in Area 2.4).
//! * COOP / COEP / CORP presence is recorded (counters + logs) for the
//!   dashboard; full cross-origin-isolation enforcement needs engine
//!   support (upstream follow-up, see the Area 2 report).
//!
//! Plus two request-side checks that need no probe:
//! * mixed content: an https document loading a plain-http subresource
//!   that the HTTPS-Only upgrader did not rewrite (exempt host / mode
//!   off) is blocked outright, matching Chrome's post-M79 behavior;
//! * SRI is NOT implementable embedder-side (no integrity metadata on
//!   `WebResourceRequest`, no response body on the pass-through path) —
//!   recorded as an engine gap + upstream issue.
//!
//! Probes fail OPEN: a failed/timeout probe stores a negative cache
//! entry and the site loads without header enforcement. Browsing must
//! never break because a probe did not answer.

use std::collections::HashMap;
use std::io::Read;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use content_security_policy::{
    CheckResult, CspList, Initiator, ParserMetadata, PolicyDisposition, PolicySource, Request,
};
use url::Url;

use crate::upgrade::HttpsUpgrader;

/// Positive cache TTL for a successful probe.
const SITE_TTL: Duration = Duration::from_secs(300);
/// Negative cache TTL (probe failed / unreachable): retry after this.
const NEG_TTL: Duration = Duration::from_secs(60);
/// Probe budget: connect 2 s, whole request 4 s.
const PROBE_CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
const PROBE_TOTAL_TIMEOUT: Duration = Duration::from_secs(4);
/// Cap on the per-session decision logs (dashboard/report).
const LOG_CAP: usize = 128;

/// Framing rule extracted from XFO + CSP frame-ancestors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameRule {
    /// No framing header seen — framing allowed (spec default).
    AllowAll,
    /// X-Frame-Options: DENY / CSP frame-ancestors 'none'.
    Deny,
    /// X-Frame-Options: SAMEORIGIN / frame-ancestors 'self'.
    SameOrigin,
    /// frame-ancestors host list (origins allowed to embed).
    Ancestors(Vec<String>),
}

/// Parsed security headers for one site (host:port key).
#[derive(Debug, Clone)]
pub struct SiteSecurity {
    /// Enforce-ready CSP list from the document response.
    pub csp: Option<CspList>,
    /// Raw CSP header (needed for nonce/hash/strict-dynamic detection).
    pub csp_raw: Option<String>,
    /// Raw frame-ancestors source strings (empty when absent).
    pub frame_ancestors: Vec<String>,
    /// X-Frame-Options value, uppercased (DENY / SAMEORIGIN / other).
    pub xfo: Option<String>,
    /// HSTS max-age + includeSubDomains from the document response.
    pub hsts: Option<(u64, bool)>,
    pub coop: bool,
    pub coep: bool,
    pub corp: bool,
}

impl SiteSecurity {
    /// The combined framing rule (XFO and frame-ancestors are OR-ed:
    /// whichever is stricter applies; here we evaluate both at check
    /// time, so we just carry the parsed inputs).
    pub fn frame_rule(&self) -> FrameRule {
        // frame-ancestors supersedes XFO when both exist (spec behavior);
        // XFO is honored when only it is present.
        if !self.frame_ancestors.is_empty() {
            ancestors_rule(&self.frame_ancestors)
        } else {
            match self.xfo.as_deref() {
                Some("DENY") => FrameRule::Deny,
                Some("SAMEORIGIN") => FrameRule::SameOrigin,
                _ => FrameRule::AllowAll,
            }
        }
    }
}

fn ancestors_rule(sources: &[String]) -> FrameRule {
    if sources.iter().any(|s| s == "'none'") {
        return FrameRule::Deny;
    }
    if sources.iter().any(|s| s == "*") {
        return FrameRule::AllowAll;
    }
    let hosts: Vec<String> = sources
        .iter()
        .filter(|s| *s != "'self'" && !s.starts_with('\''))
        .map(|s| {
            // host-source: [scheme://] host [:port] [path] — keep the host
            let no_scheme = match s.split_once("://") {
                Some((_, rest)) => rest,
                None => s,
            };
            no_scheme.split('/').next().unwrap_or(no_scheme).to_string()
        })
        .filter(|s| !s.is_empty())
        .collect();
    if sources.iter().any(|s| s == "'self'") && hosts.is_empty() {
        FrameRule::SameOrigin
    } else {
        FrameRule::Ancestors(hosts)
    }
}

struct CacheEntry {
    sec: Option<Arc<SiteSecurity>>,
    fetched: Instant,
}

/// The embedder-side security guard. One per PrivacyHost.
pub struct SecurityGuard {
    enabled: AtomicBool,
    agent: ureq::Agent,
    upgrader: Arc<HttpsUpgrader>,
    /// host:port → probed headers.
    cache: Mutex<HashMap<String, CacheEntry>>,
    pub probes: AtomicU64,
    pub frames_blocked: AtomicU64,
    pub csp_blocked: AtomicU64,
    pub mixed_content_blocked: AtomicU64,
    pub hsts_learned: AtomicU64,
    pub coop_observed: AtomicU64,
    pub coep_observed: AtomicU64,
    pub corp_observed: AtomicU64,
    pub frame_log: Mutex<Vec<String>>,
    pub csp_block_log: Mutex<Vec<String>>,
}

impl Default for SecurityGuard {
    fn default() -> Self {
        Self::new(Arc::new(HttpsUpgrader::new()))
    }
}

impl SecurityGuard {
    pub fn new(upgrader: Arc<HttpsUpgrader>) -> Self {
        #[allow(clippy::arc_with_non_send_sync)]
        Self {
            enabled: AtomicBool::new(true),
            agent: ureq::AgentBuilder::new()
                .timeout_connect(PROBE_CONNECT_TIMEOUT)
                .timeout(PROBE_TOTAL_TIMEOUT)
                .redirects(5)
                .user_agent("brows12-security/2.7")
                .build(),
            upgrader,
            cache: Mutex::new(HashMap::new()),
            probes: AtomicU64::new(0),
            frames_blocked: AtomicU64::new(0),
            csp_blocked: AtomicU64::new(0),
            mixed_content_blocked: AtomicU64::new(0),
            hsts_learned: AtomicU64::new(0),
            coop_observed: AtomicU64::new(0),
            coep_observed: AtomicU64::new(0),
            corp_observed: AtomicU64::new(0),
            frame_log: Mutex::new(Vec::new()),
            csp_block_log: Mutex::new(Vec::new()),
        }
    }

    pub fn set_enabled(&self, on: bool) {
        self.enabled.store(on, Ordering::Relaxed);
    }

    fn enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    /// Cache key for a URL: `host:port` (precision beats the registrable
    /// domain here: CSP is a per-response artifact).
    fn cache_key(url: &Url) -> Option<String> {
        let host = url.host_str()?.to_ascii_lowercase();
        let port = url.port_or_known_default()?;
        Some(format!("{host}:{port}"))
    }

    /// Probed security headers for `url`'s site; probes on miss
    /// (synchronous, cached). Returns None when the site is not
    /// probeable (non-http scheme) or the probe failed (negative cache).
    pub fn site_security_for(&self, url: &str) -> Option<Arc<SiteSecurity>> {
        if !self.enabled() {
            return None;
        }
        let parsed = Url::parse(url).ok()?;
        if !matches!(parsed.scheme(), "http" | "https") {
            return None;
        }
        let key = Self::cache_key(&parsed)?;
        if let Some(entry) = self.cache.lock().unwrap().get(&key) {
            let ttl = if entry.sec.is_some() { SITE_TTL } else { NEG_TTL };
            if entry.fetched.elapsed() < ttl {
                return entry.sec.clone();
            }
        }
        let sec = self.probe(&parsed);
        self.cache
            .lock()
            .unwrap()
            .insert(key, CacheEntry { sec: sec.clone(), fetched: Instant::now() });
        sec
    }

    /// Probe the document (called for every main-frame navigation before
    /// pass-through): caches CSP / framing / HSTS / COOP-COEP-CORP and
    /// feeds runtime HSTS into the HTTPS-Only upgrader.
    pub fn probe_document(&self, url: &str) {
        let _ = self.site_security_for(url);
    }

    fn probe(&self, url: &Url) -> Option<Arc<SiteSecurity>> {
        self.probes.fetch_add(1, Ordering::Relaxed);
        let resp = self.agent.get(url.as_str()).call().ok()?;
        let csp = resp.header("content-security-policy").map(|s| s.to_string());
        let xfo = resp.header("x-frame-options").map(|s| s.to_ascii_uppercase());
        let hsts: Option<(u64, bool)> =
            resp.header("strict-transport-security").and_then(parse_hsts);
        let coop = resp.header("cross-origin-opener-policy").is_some();
        let coep = resp.header("cross-origin-embedder-policy").is_some();
        let corp = resp.header("cross-origin-resource-policy").is_some();
        // Header inspection only: read a small chunk, then drop the
        // connection. The page itself is fetched by Servo separately.
        let mut reader = resp.into_reader();
        let mut sink = [0u8; 512];
        let _ = reader.read(&mut sink);
        drop(reader);

        if coop {
            self.coop_observed.fetch_add(1, Ordering::Relaxed);
        }
        if coep {
            self.coep_observed.fetch_add(1, Ordering::Relaxed);
        }
        if corp {
            self.corp_observed.fetch_add(1, Ordering::Relaxed);
        }

        let csp_raw = csp;
        let csp_list = csp_raw
            .as_deref()
            .map(|raw| CspList::parse(raw, PolicySource::Header, PolicyDisposition::Enforce));
        // Runtime HSTS learning (the 2.4 gap, closed probe-side).
        if let Some((max_age, subdomains)) = hsts {
            let host = url.host_str().unwrap_or_default().to_string();
            if !host.is_empty() {
                self.upgrader.record_hsts(&host, max_age, subdomains);
                self.hsts_learned.fetch_add(1, Ordering::Relaxed);
            }
        }

        Some(Arc::new(SiteSecurity {
            frame_ancestors: csp_raw
                .as_deref()
                .map(frame_ancestors_from_header)
                .unwrap_or_default(),
            csp: csp_list,
            csp_raw,
            xfo,
            hsts,
            coop,
            coep,
            corp,
        }))
    }

    /// Frame-guard: is `frame_url` allowed to be embedded by `parent_url`?
    /// Returns Some(reason) when embedding is forbidden. Same-origin
    /// frames always pass; cross-origin frames consult the frame
    /// target's XFO / frame-ancestors (probed + cached).
    pub fn check_frame(&self, parent_url: &str, frame_url: &str) -> Option<String> {
        if !self.enabled() {
            return None;
        }
        let parent = Url::parse(parent_url).ok()?;
        let frame = Url::parse(frame_url).ok()?;
        if !matches!(frame.scheme(), "http" | "https") {
            return None;
        }
        if parent.origin() == frame.origin() {
            return None; // same-origin embedding is always allowed
        }
        let sec = self.site_security_for(frame_url)?;
        decide_frame(&sec, parent.as_str(), frame.as_str())
    }

    /// CSP enforcement for a subresource requested by a document:
    /// consults the document site's probed CSP with spec-accurate
    /// matching (`should_request_be_blocked`). Returns Some(reason) when
    /// the request violates the policy.
    pub fn check_subresource(
        &self,
        doc_url: &str,
        req_url: &str,
        destination: &content_security_policy::Destination,
    ) -> Option<String> {
        if !self.enabled() {
            return None;
        }
        let doc = Url::parse(doc_url).ok()?;
        if !matches!(doc.scheme(), "http" | "https") {
            return None;
        }
        let sec = self.site_security_for(doc_url)?;
        let csp = sec.csp.as_ref()?;
        // Nonce / hash / strict-dynamic policies authorize loads per-ELEMENT
        // metadata the embedder cannot see (the request carries no nonce),
        // so host-source enforcement there would over-block. Chrome-parity:
        // skip those for script/style; enforce everything else.
        if matches!(
            destination,
            content_security_policy::Destination::Script
                | content_security_policy::Destination::Style
        ) && sec.csp_raw.as_deref().is_some_and(|raw| {
            raw.contains("'nonce-") || raw.contains("'sha") || raw.contains("'strict-dynamic'")
        }) {
            return None;
        }
        evaluate_csp(csp, &doc, req_url, *destination)
    }

    /// Mixed-content decision (request-side; no probe needed). True when
    /// an https document asks for a plain-http subresource.
    pub fn check_mixed_content(&self, doc_url: &str, req_url: &str) -> bool {
        if !self.enabled() {
            return false;
        }
        let (Ok(doc), Ok(req)) = (Url::parse(doc_url), Url::parse(req_url)) else {
            return false;
        };
        doc.scheme() == "https" && req.scheme() == "http"
    }

    pub fn record_frame_blocked(&self, frame_url: &str, reason: &str) {
        self.frames_blocked.fetch_add(1, Ordering::Relaxed);
        let mut log = self.frame_log.lock().unwrap();
        if log.len() < LOG_CAP {
            log.push(format!("{frame_url} :: {reason}"));
        }
    }

    pub fn record_csp_blocked(&self, url: &str, reason: &str) {
        self.csp_blocked.fetch_add(1, Ordering::Relaxed);
        let mut log = self.csp_block_log.lock().unwrap();
        if log.len() < LOG_CAP {
            log.push(format!("{url} :: {reason}"));
        }
    }

    pub fn record_mixed_content_blocked(&self, url: &str) {
        self.mixed_content_blocked.fetch_add(1, Ordering::Relaxed);
        let mut log = self.frame_log.lock().unwrap();
        if log.len() < LOG_CAP {
            log.push(format!("mixed content blocked: {url}"));
        }
    }

    /// Dashboard snapshot (Area 2.8 consumes this).
    pub fn summary(&self) -> SecuritySummary {
        SecuritySummary {
            probes: self.probes.load(Ordering::Relaxed),
            frames_blocked: self.frames_blocked.load(Ordering::Relaxed),
            csp_blocked: self.csp_blocked.load(Ordering::Relaxed),
            mixed_content_blocked: self.mixed_content_blocked.load(Ordering::Relaxed),
            hsts_learned: self.hsts_learned.load(Ordering::Relaxed),
            coop_observed: self.coop_observed.load(Ordering::Relaxed),
            coep_observed: self.coep_observed.load(Ordering::Relaxed),
            corp_observed: self.corp_observed.load(Ordering::Relaxed),
            frame_log: self.frame_log.lock().unwrap().clone(),
            csp_block_log: self.csp_block_log.lock().unwrap().clone(),
        }
    }
}

/// Serializable snapshot for the JSON report / dashboard.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SecuritySummary {
    pub probes: u64,
    pub frames_blocked: u64,
    pub csp_blocked: u64,
    pub mixed_content_blocked: u64,
    pub hsts_learned: u64,
    pub coop_observed: u64,
    pub coep_observed: u64,
    pub corp_observed: u64,
    pub frame_log: Vec<String>,
    pub csp_block_log: Vec<String>,
}

/// Frame decision from parsed headers (free function for tests).
pub fn decide_frame(sec: &SiteSecurity, parent_url: &str, frame_url: &str) -> Option<String> {
    let parent = Url::parse(parent_url).ok()?;
    let frame = Url::parse(frame_url).ok()?;
    match sec.frame_rule() {
        FrameRule::AllowAll => None,
        FrameRule::Deny => Some("frame denied by site policy".to_string()),
        FrameRule::SameOrigin => {
            (parent.origin() != frame.origin()).then(|| "frame same-origin only".to_string())
        }
        FrameRule::Ancestors(hosts) => {
            let parent_host = parent.host_str().unwrap_or_default().to_ascii_lowercase();
            let allowed = hosts.iter().any(|h| {
                // entry may carry scheme://host or host[:port]; compare host
                let host = h
                    .rsplit_once("://")
                    .map(|(_, rest)| rest)
                    .unwrap_or(h)
                    .split(':')
                    .next()
                    .unwrap_or(h)
                    .to_ascii_lowercase();
                !host.is_empty() && host == parent_host
            });
            (!allowed).then(|| format!("frame-ancestors does not include {}", parent_host))
        }
    }
}

/// Spec-accurate CSP request check (free function for tests).
pub fn evaluate_csp(
    csp: &CspList,
    doc_url: &Url,
    req_url: &str,
    destination: content_security_policy::Destination,
) -> Option<String> {
    let req = Url::parse(req_url).ok()?;
    let request = Request {
        url: req.clone(),
        current_url: req,
        origin: doc_url.origin(),
        redirect_count: 0,
        destination,
        initiator: Initiator::None,
        nonce: String::new(),
        integrity_metadata: String::new(),
        parser_metadata: ParserMetadata::None,
    };
    let (result, _) = csp.should_request_be_blocked(&request);
    (result == CheckResult::Blocked).then(|| "CSP violation".to_string())
}

/// Parse `Strict-Transport-Security` → (max-age, includeSubDomains).
pub fn parse_hsts(value: &str) -> Option<(u64, bool)> {
    let mut max_age = None;
    let mut include = false;
    for part in value.split(';') {
        let part = part.trim();
        if let Some(rest) = part.strip_prefix("max-age=") {
            max_age = rest.trim().parse::<u64>().ok();
        } else if part.eq_ignore_ascii_case("includesubdomains") {
            include = true;
        }
    }
    max_age.map(|m| (m, include))
}

/// Extract frame-ancestors source expressions from a raw CSP header.
pub fn frame_ancestors_from_header(raw: &str) -> Vec<String> {
    for directive in raw.split(';') {
        let mut tokens = directive.split_whitespace();
        if matches!(tokens.next(), Some(t) if t.eq_ignore_ascii_case("frame-ancestors")) {
            return tokens.map(|s| s.to_string()).collect();
        }
    }
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::net::TcpListener;
    use std::thread::JoinHandle;

    /// Serve `response` bytes to the first `n` connections on a random
    /// loopback port; returns the base URL.
    fn serve_raw(response: &'static str, n: usize) -> (String, JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().unwrap().port();
        let handle = std::thread::spawn(move || {
            for _ in 0..n {
                let Ok((mut sock, _)) = listener.accept() else { return };
                let mut buf = [0u8; 2048];
                let _ = sock.read(&mut buf); // drain the request head
                let _ = sock.write_all(response.as_bytes());
                let _ = sock.flush();
            }
        });
        (format!("http://127.0.0.1:{port}"), handle)
    }

    const HEADERS_CSP_XFO_HSTS: &str = "HTTP/1.1 200 OK\r\n\
        Content-Type: text/html\r\n\
        Content-Security-Policy: script-src 'self'; frame-ancestors 'self'; upgrade-insecure-requests\r\n\
        X-Frame-Options: DENY\r\n\
        Strict-Transport-Security: max-age=31536000; includeSubDomains\r\n\
        Cross-Origin-Opener-Policy: same-origin\r\n\
        Cross-Origin-Embedder-Policy: require-corp\r\n\
        Cross-Origin-Resource-Policy: same-origin\r\n\
        Content-Length: 2\r\n\r\nok\r\n";

    #[test]
    fn parses_hsts() {
        assert_eq!(parse_hsts("max-age=63072000"), Some((63_072_000, false)));
        assert_eq!(parse_hsts("max-age=31536000 ; includeSubDomains"), Some((31_536_000, true)));
        assert_eq!(parse_hsts("max-age=0"), Some((0, false)));
        assert_eq!(parse_hsts("garbage"), None);
    }

    #[test]
    fn parses_frame_ancestors() {
        let fa = frame_ancestors_from_header(
            "default-src 'self'; frame-ancestors https://a.example *.b.example; script-src x",
        );
        assert_eq!(fa, vec!["https://a.example", "*.b.example"]);
        assert!(frame_ancestors_from_header("script-src 'self'").is_empty());
        assert_eq!(ancestors_rule(&["'none'".into()]), FrameRule::Deny);
        assert_eq!(ancestors_rule(&["'self'".into()]), FrameRule::SameOrigin);
        assert_eq!(ancestors_rule(&["*".into()]), FrameRule::AllowAll);
        let hosts = ancestors_rule(&["https://a.example".into(), "b.example:8080".into()]);
        assert_eq!(hosts, FrameRule::Ancestors(vec!["a.example".into(), "b.example:8080".into()]));
    }

    #[test]
    fn frame_decisions_match_spec() {
        let mut sec = SiteSecurity {
            csp: None,
            csp_raw: None,
            frame_ancestors: vec![],
            xfo: Some("DENY".into()),
            hsts: None,
            coop: false,
            coep: false,
            corp: false,
        };
        let parent = "https://evil.example/page";
        let frame = "https://victim.example/x";
        assert!(decide_frame(&sec, parent, frame).is_some());
        // Spec: XFO DENY blocks ALL framing — same-origin included.
        assert!(decide_frame(&sec, "https://victim.example/other", frame).is_some());

        sec.xfo = Some("SAMEORIGIN".into());
        assert!(decide_frame(&sec, parent, frame).is_some());

        sec.xfo = None;
        sec.frame_ancestors = vec!["https://embed.example".into()];
        assert!(decide_frame(&sec, parent, frame).is_some());
        assert!(decide_frame(&sec, "https://embed.example/p", frame).is_none());

        sec.frame_ancestors = vec!["'none'".into()];
        assert!(decide_frame(&sec, "https://embed.example/p", frame).is_some());

        sec.frame_ancestors = vec!["*".into()];
        assert!(decide_frame(&sec, parent, frame).is_none());
    }

    #[test]
    fn csp_blocks_cross_site_script_and_allows_self() {
        let csp = CspList::parse(
            "script-src 'self'; style-src 'self' https://cdn.good.example",
            PolicySource::Header,
            PolicyDisposition::Enforce,
        );
        let doc = Url::parse("https://site.example/").unwrap();
        let bad = evaluate_csp(
            &csp,
            &doc,
            "https://evil.example/tracker.js",
            content_security_policy::Destination::Script,
        );
        assert!(bad.is_some(), "cross-site script must violate script-src 'self'");
        let good = evaluate_csp(
            &csp,
            &doc,
            "https://site.example/app.js",
            content_security_policy::Destination::Script,
        );
        assert!(good.is_none(), "same-origin script must pass");
        let cdn = evaluate_csp(
            &csp,
            &doc,
            "https://cdn.good.example/x.css",
            content_security_policy::Destination::Style,
        );
        assert!(cdn.is_none(), "allow-listed stylesheet host must pass");
        // Images are not governed by script-src — allowed.
        let img = evaluate_csp(
            &csp,
            &doc,
            "https://evil.example/pixel.png",
            content_security_policy::Destination::Image,
        );
        assert!(img.is_none(), "img not covered by script-src");
    }

    #[test]
    fn probe_parses_headers_and_learns_hsts() {
        let (base, _h) = serve_raw(HEADERS_CSP_XFO_HSTS, 2);
        let upgrader = Arc::new(HttpsUpgrader::new());
        let guard = SecurityGuard::new(upgrader);
        guard.probe_document(&format!("{base}/page"));
        let sec = guard.site_security_for(&format!("{base}/page")).expect("cached");
        assert!(sec.csp.is_some(), "CSP must parse");
        assert_eq!(sec.xfo.as_deref(), Some("DENY"));
        assert_eq!(sec.hsts, Some((31_536_000, true)));
        assert!(sec.coop && sec.coep && sec.corp);
        assert_eq!(guard.hsts_learned.load(Ordering::Relaxed), 1);
        assert_eq!(guard.coop_observed.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn frame_check_blocks_via_probe() {
        let (base, _h) = serve_raw(HEADERS_CSP_XFO_HSTS, 2);
        let guard = SecurityGuard::default();
        let reason = guard
            .check_frame("https://innocent.example/", &format!("{base}/adframe"))
            .expect("cross-origin embed of 'self'-only page must be blocked");
        // frame-ancestors 'self' (present alongside XFO:DENY) supersedes
        // XFO per spec → "same-origin only" is the applicable reason.
        assert!(reason.contains("same-origin"), "unexpected reason: {reason}");
    }

    #[test]
    fn failed_probe_fails_open_and_negative_caches() {
        // Port with no listener → probe fails → None, no enforcement.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener); // closed port
        let guard = SecurityGuard::default();
        assert!(guard.site_security_for(&format!("http://127.0.0.1:{port}/x")).is_none());
        assert!(guard
            .check_frame("https://a.example/", &format!("http://127.0.0.1:{port}/f"))
            .is_none());
        assert!(guard
            .check_subresource(
                &format!("http://127.0.0.1:{port}/"),
                "http://127.0.0.1:{port}/s.js",
                &content_security_policy::Destination::Script,
            )
            .is_none());
    }

    #[test]
    fn mixed_content_decision() {
        let guard = SecurityGuard::default();
        assert!(guard.check_mixed_content("https://good.example/", "http://cdn.example/x.js"));
        assert!(!guard.check_mixed_content("http://good.example/", "http://cdn.example/x.js"));
        assert!(!guard.check_mixed_content("https://good.example/", "https://cdn.example/x.js"));
    }
}
