# Phase 4 — Focus Area 2 Report: Strongest-in-Class Ads, Privacy & Security

**Scope:** the eight sub-items of Focus Area 2 — network-level ad/tracker
blocking (2.1), anti-fingerprinting (2.2), pop-up/interstitial blocking
(2.3), HTTPS-Only + HSTS (2.4), DoH (2.5), cookie isolation / Total Cookie
Protection (2.6), security hardening (2.7), and the privacy dashboard data
layer (2.8). Per mission rules, every sub-item was committed and pushed
individually (`e6f13fc`, `d0c5026`, `573d9c5`, `261029c`, `a23c48c`,
`22e6857`, `3375534`, `fe5d953`).

**Verification method:** every item carries (a) unit tests in-tree, and
(b) at least one side-by-side composite with Chromium ground truth under
`screenshots/v2-servo/phase4/` (`area2_1_*`, `area2_2_*`, `area2_3_*`,
`area2_4_*`, `area2_7_*`), produced by the same Playwright-Chromium
pipeline as Areas 0–1. Loopback fixtures use distinct 127.0.0.x hosts as
distinct "sites" so cross-site semantics (third-party cookies, framing,
CSP origins) are exercised for real.

---

## 2.1 Network-level ad & tracker blocking — DONE ✔

**Engine:** Brave's `adblock` crate (the same matcher Brave ships) over the
**full 2026-10 lists**: EasyList 2.09 MB, EasyPrivacy 1.51 MB, uBO filters
474 KB, uBO Privacy 184 KB + a brows12 policy list (universal
`utm_*`/`fbclid`/`gclid`/… stripping — ~43 params; uBO deliberately ships
no universal utm rules). **143,204 rules** compiled in 148–205 ms on the
2-core box, embedded via `include_str!` (no runtime download, no
first-run gap).

**Interception point:** `WebViewDelegate::load_web_resource` — every
request Servo is about to make (post-CORS, with destination metadata)
receives a full `Verdict` **before a single byte or DNS query leaves the
process**. Verdict surface:

* blocked + `$redirect` → the real replacement resource body is served
  (uBO-compatible resource bundle: `noopjs`, `1x1.gif`, `noopmp3/mp4`
  generated via ffmpeg, `google-ima`, `googlesyndication_adsbygoogle`,
  `chartbeat`, `click2load.html`, + 8 templated scriptlets);
* blocked otherwise → empty 200 (page JS sees a settled resource);
* allowed + `$removeparam` → 301 to the stripped URL;
* allowed + `$csp` (documents) → meta-CSP injection;
* cosmetic filtering: full uBO two-phase protocol — URL-specific selectors
  from `url_cosmetic_resources` + generic class/id rules matched in Rust
  against the live DOM's attributes (DOM_INFO_JS probe, 4000-attr cap) and
  hidden via a user stylesheet;
* scriptlet injection: uBO-compatible bundle installed as a user script
  per navigation.

**UPSTREAM FINDING (filed in Area 1.7 batch):** `adblock` 0.13.3 parses
`$removeparam` rules but never matches them (minimal repro in
`privacy/src/removeparam.rs` docs). Worked around with a own uBO-subset
matcher (pattern `*`/`||host^`, domain include/exclude) — 43 brows12
extras rules verified stripping.

**Verification:** ad-heavy fixtures + real sites (thesun.co.uk,
speedtest.net): zero ad/tracker requests leave the browser; counters and
blocked-request sample in the JSON report; side-by-side composites show
ad slots collapsed on both engines.

## 2.2 Anti-fingerprinting — DONE ✔ (beyond Brave "Standard" in 4 areas)

`privacy/src/fingerprint.rs` ships a per-session, per-site deterministic
noise/spoof layer installed once via the UserContentManager so **every**
document (main frame + iframes) is patched before any page script runs.

Covered surfaces: Canvas2D export noise (getImageData/toDataURL/toBlob),
AudioBuffer.getChannelData + AnalyserNode (Proxy-wrapped index reads —
defeats the write-after-read probe pattern), WebGL vendor/renderer spoof +
readPixels noise, measureText jitter (font enumeration via widths fails),
hardwareConcurrency/deviceMemory/maxTouchPoints/webdriver/languages/
plugins, battery stub, connection type, enumerateDevices `[]`, gamepad/
USB/BT/Serial/MIDI lockdown, WebRTC stub (no srflx candidates —
future-proofing; Servo 0.6 has no RTCPeerConnection), and Strict-only:
screen geometry/depth + timezone generalization (UTC).

Key design: noise is **stable within a session** (page self-consistency
preserved — canvas_hash_1 == canvas_hash_2) and **random across
sessions** (two processes → different hashes, session linkability
broken). Debugging journey (regex escape bug, nested wrapper
destabilization, audio write-after-read) is documented in the worklog.

**Verification:** fixture self-report + Chrome side-by-side
(`area2_2_fingerprint_side_by_side.png`).

## 2.3 Pop-up / pop-under / interstitial blocking — DONE ✔

* All auxiliary webviews created via `request_create_new` are blocked
  (covers `window.open()` AND pop-unders — same mechanism opened behind
  the current window). Stricter than Chrome's gesture heuristic because
  the embedder cannot observe user activation; per-site exceptions are a
  UI-layer follow-up.
* Interstitial redirect funnels: an 8-second hop-rate guard cuts the
  chain at the 5th distinct registrable domain (hops compared against the
  last recorded hop; the initial load is not a hop). Fixture: 5 hops
  across 5 loopback IPs → cut, counter + log recorded.

**Verification:** `area2_3_popup_side_by_side.png` + unit tests.

## 2.4 HTTPS-Only + HSTS — DONE ✔

Default mode **HttpsOnly** (runtime-switchable Off/Upgradable/HttpsOnly;
the old `set_mode` was a silent no-op — now an AtomicU8 with tests).
Exemptions: IP literals (local fixtures keep working), localhost,
`.onion`, `.test`, + user per-site exceptions (registrable-domain
semantics). Shipped with a curated 48-domain HSTS preload slice
(top-traffic domains shipping includeSubDomains, Chromium preload list
2026-10) with parent-domain matching.

Three Servo integration designs were tried; the two obvious ones fail
(deny+reload and deny+deferred-location.replace both leave a blank
document — any deny-poisoned initial navigation breaks render in 0.6).
**Shipped:** plain-HTTP documents are answered with a meta-refresh
upgrade page (issues a real navigation that renders); plain-HTTP
subresources get a 301 to https (re-request re-enters the hook with
https). Runtime HSTS *learning* from response headers was impossible in
2.4 (no response visibility) — **closed in 2.7** via the header probe.

**Verification:** `http://example.com` → final URL https, title intact,
upgrades=1; IP-literal fixture stays http;
`area2_4_https_side_by_side.png` (Chrome stays on http; brows12
upgrades). neverssl.com noted: its https endpoint fails in Servo 0.6 TLS
regardless of path (pre-existing engine limitation).

## 2.5 DNS-over-HTTPS + CNAME cloaking — DONE ✔

`privacy/src/doh.rs`: RFC 8484 DoH client with three built-in providers
(Cloudflare, Quad9, Mullvad) + custom URL, runtime-switchable, 5-min
cache, failure counter. Every brows12-side resolution goes over DoH;
CNAME chains are classified by the PolicyEngine (registrable-domain
comparison) — cloaked hosts feed the 2.6 cookie policy and the 2.8
dashboard.

Honest notes: Servo's own resolver stays getaddrinfo (no embedder hook in
0.6) — upstream issue filed as follow-up; DoT (RFC 7858) deferred (DoH
provides the same encryption over the HTTPS path already in use).

## 2.6 Cookie isolation / Total Cookie Protection — DONE ✔ (decision layer)

`brows12-storage` CookieJar gained the policy-aware entry point
`set_from_header_with_policy` returning a typed `CookieDecision`:
`Allow` (CHIPS opt-in semantics), `PartitionAll` (**Total Cookie
Protection** — every third-party cookie force-partitioned under the
top-level site), `Reject` (third-party Set-Cookie dropped). `__Host-`
prefix rules enforced in every mode (RFC 6265bis §4.1.3.2). CNAME-cloaking
tie-in: cloaked hosts (2.5) classify as the cloak target's site, so
cloaked trackers partition/reject as third parties.

**Empirical Servo gap (probed live):** Servo 0.6's jar accepts the
`Partitioned` attribute but stores cookies **unpartitioned**, and accepts
+ replays third-party iframe cookies unpartitioned — Chrome today
partitions/blocks by default. Wiring the decision engine into Servo's
in-process jar requires an engine change (cookie storage is inside the
net stack, invisible to the embedder) — **reference implementation
shipped, upstream issue to file**.

## 2.7 Security hardening — DONE ✔ (the engine never enforced these)

Empirical audit of the 0.6.0 crates: the net/script crates never read
`Content-Security-Policy`, `X-Frame-Options`, `Strict-Transport-Security`,
or COOP/COEP/CORP, and the embedder cannot see pass-through response
headers. `privacy/src/security.rs` closes the gap the only reliable
embedder-side way — a cached **header probe** per main-frame document and
cross-site frame target (fail-open, 5-min positive / 60-s negative
cache), then request-side enforcement in `load_web_resource`:

* **CSP enforcement:** parsed with the Servo-team
  `content_security_policy` crate (spec-accurate matching — host
  sources, wildcards, paths); every later subresource of that document is
  evaluated with `should_request_be_blocked` **before** Servo fetches it.
  Nonce/hash/strict-dynamic policies are skipped for script/style (the
  request carries no element metadata — avoids over-blocking, matching
  what the embedder can legitimately know).
* **Clickjacking defense:** X-Frame-Options + CSP frame-ancestors
  (frame-ancestors supersedes XFO per spec; DENY blocks same-origin too).
  Same-origin frames always pass; cross-origin frames embed only if the
  frame target's own headers allow.
* **Runtime HSTS learning:** probe responses feed
  `HttpsUpgrader::record_hsts` — closes the 2.4 gap without an engine
  hook.
* **Mixed content:** https document + plain-http subresource that the
  upgrader did not rewrite (exempt host / mode off) is blocked outright —
  Chrome post-M79 semantics.
* **COOP/COEP/CORP:** presence recorded (counters + logs) for the
  dashboard; full cross-origin-isolation enforcement needs engine support
  (upstream).
* **SRI:** NOT implementable embedder-side (no integrity metadata on
  `WebResourceRequest`, no response body visibility on the pass-through
  path) — honest gap, upstream issue.

**Verification:** 46 privacy tests (incl. raw-TcpListener probe tests,
DENY-blocks-same-origin, fail-open negative cache); E2E fixture
(`scripts/phase4_area2_7.py`, three loopback hosts): main doc with
`Content-Security-Policy: script-src 'self'` + HSTS header, cross-site
evil.js, XFO:DENY frame target → probes=2, csp_blocked=1 (evil.js
denied), frames_blocked=1, hsts_learned=1, page title intact;
**Chrome parity** (Playwright Chromium blocks the same script via its own
CSP engine — identical rendered outcome,
`area2_7_security_side_by_side.png`); real-site sanity: example.com
loads complete, probe=1, zero false positives.

## 2.8 Privacy dashboard (data layer) — DONE ✔

`servo-host/src/dashboard.rs`: one serializable `PrivacyDashboard`
aggregating every counter above (ads/trackers blocked + capped sample,
redirects/params/csp/scriptlets/cosmetics, fingerprint coverage, popups +
redirect chains, https upgrades + HSTS hits + learned, DoH queries +
cloaks, cookie decisions, security-guard counters + decision logs) —
29 fields. Exposed two ways: embedded in the headless/perf report JSON
(`privacy.dashboard`) and a callable API (`dashboard_snapshot`,
`dashboard_json`) for the future UI. UI rendering is intentionally out of
scope (data-layer-only per mission). Cookie decisions are typed
end-to-end (`CookieDecision` from the storage decision layer) with a
`CookieDecisionSink` feeding the dashboard.

---

## Honest gaps & upstream follow-ups (consolidated)

1. **Cookie-jar engine wiring (2.6):** decision layer is complete and
   typed; Servo's in-process jar cannot be reached from the embedder.
2. **SRI (2.7):** needs `integrity_metadata` on the embedder request and
  /or a response-inspection hook.
3. **Cross-origin isolation (2.7):** COOP/COEP observed, not enforced;
   requires browsing-context-group semantics in the constellation.
4. **Inline-script CSP (2.7):** nonce/hash authorization is JS-level;
   only the network path is enforceable embedder-side.
5. **adblock `$removeparam` (2.1):** upstream crate parses but never
   matches — brows12 ships its own matcher.
6. **Servo resolver (2.5):** no embedder DNS hook — DoH covers
   brows12-side lookups only.
7. **Multiple CSP headers:** first header only (ureq limitation).
8. **Servo 0.6 TLS:** some https endpoints (e.g. neverssl.com) fail at
   the engine level regardless of embedder behavior.

## Verdict

| # | Item | State |
|---|------|-------|
| 2.1 | Network ad/tracker blocking + cosmetics + scriptlets | DONE ✔ |
| 2.2 | Anti-fingerprinting | DONE ✔ |
| 2.3 | Pop-up / pop-under / interstitial blocking | DONE ✔ |
| 2.4 | HTTPS-Only + HSTS (preload + runtime learning via 2.7) | DONE ✔ |
| 2.5 | DoH + CNAME-cloak detection | DONE ✔ |
| 2.6 | Cookie isolation / Total Cookie Protection | DONE ✔ (decision layer; engine wiring = upstream) |
| 2.7 | CSP / XFO / HSTS / mixed content / COOP-COEP-CORP / SRI | DONE ✔ (SRI = documented upstream gap) |
| 2.8 | Privacy dashboard data layer | DONE ✔ |
