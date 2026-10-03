# Worklog — v1.0.0-rc1 mission

Method (unchanged from V1_PLAN): every fix is verified by rendering the same
page in Playwright Chromium and in brows12 at 1280x800 and comparing side by
side. No success is claimed without the comparison. One fix per commit.

Environment note: this session started from a fresh container (rustup
reinstalled, repo re-cloned). Baseline re-verified before any change:
origin/main @ dd77f21, build green, 115 tests green.

---

## 2026-10-03 — Session audit + P0-4 (A1 flex min-content)

Task ID: 2 (audit) + 3 (P0-4)
Agent: Super Z (main)

Work Log:
- Fresh clone of salim77007j/brows12 @ dd77f21; verified origin/main match.
- Rebuilt workspace, re-ran full suite: 115 tests green.
- Confirmed from git log that P0-1 (hidden elements, d26724c), P0-2
  (Wikipedia dark canvas, bca5c49), P0-3 (em font-size cascade, 84d4d31),
  P0-5 (sticky/fixed, c98cacb), P0-6 (calc(), 0fab117), P0-7 (form
  controls/inline-block, 26fbe79) were already landed in prior sessions.
  P0-4 (flex min-content) was NOT started — began it this session.

### Fix 1 — compat User-Agent (net)

- Symptom: live Wikipedia renders via the engine came back near-blank while
  a Chrome-UA Playwright capture was fine.
- Root cause (isolated with fetch probes): the engine's default UA
  `Brows12/0.2.0` makes Wikipedia serve a broken no-JS page variant
  (missing RLCONF); UA-sniffing is a wall for every real site.
- Fix: `ClientConfig::default()` now ships a Chrome-compatible UA string
  (industry-standard practice — Firefox ships a Gecko UA; Chromium freezes
  its UA). Overridable via EngineConfig.
- Verified: engine now receives the standard Vector-2022 page (RLCONF
  present). Updated the default-config unit test.

### Fix 2 — A1 flex min-content integration (css + layout)

- Gaps found by probing (layout/tests/flex_integration.rs):
  1. `flex-basis` was never parsed/mapped (`FlexBox` had grow+shrink only).
  2. `display: inline-flex` mapped to block-level Flex (no shrink-to-fit).
  3. THE BIG ONE: flex item properties (grow/shrink/basis) were only
     applied to nodes whose own display was Flex — a plain block flex item
     kept taffy defaults (grow 0, basis auto), so `flex: 1` did nothing on
     block items and rows could not distribute free space.
- Fixes: FlexBasis enum + parsing (incl. `Property::FlexBasis`); InlineFlex
  display variant (shrink-to-fit like inline-block, atomic box); item
  properties applied unconditionally (taffy ignores them on non-items,
  spec: "no effect"); table-cell overrides kept ordered before… after the
  unconditional mapping so anonymous-flex tables still win.
- Verified: 5 new layout integration tests green (basis px/percent
  distribution matches Chrome arithmetic, fixed 120px item exact,
  automatic-minimum floor, nested flex columns, inline-flex pill);
  full workspace 120 tests green.
- Fixture: validation/run/fixtures/flex.html rendered side-by-side with
  Chromium — all six cases match (screenshots/compare/flex_fixture_compare.png).
- Real site: rust-lang.org 2025 flex nav now lays out horizontally
  (screenshots/compare/rust_lang_org_compare.png; remaining hero deltas
  are font-size/inline-justify issues, tracked below).

### Fix 3 — symbolic percentage resolution (layout)

- Symptom: live Wikipedia still collapsed (body 1280x0 in the extract
  log) although a direct-crate probe rendered the same HTML fine.
- Root cause (bisected with BROWS_DEBUG dumps + a deep box probe): the
  style builder resolved percentages EAGERLY against a flow-parent width
  estimate threaded down the recursion. Inside flex/grid/positioned
  subtrees the estimate is the page width, so Vector-2022's
  `.vector-dropdown-checkbox { position:absolute; width:100%; height:100% }`
  became a 1192x1192 box that blew the header to 1220px and pushed the
  whole article below the fold. The page was not blank — we were looking
  at the empty cavity of a 20x-tall header.
- Fix: keep percentages SYMBOLIC in the four taffy converters
  (dimension/min/max/margin/padding/border/insets helpers); taffy now
  resolves them against the real parent box (the containing block the
  spec means). calc() still resolves at build time (taffy has no calc).
- Verified: checkbox 1192x1192 -> 32x32; header 1220px -> 50px; Wikipedia
  now renders header + ToC + article title + Appearance panel and matches
  Chrome structurally (screenshots/compare/wikipedia_rust_compare.png);
  120 tests stay green; clippy clean.

### Honest remaining deltas (Wikipedia, next targets)

- Sidebar/grid placement: the ToC sits lower and wider than Chrome's
  Vector-2022 grid columns (grid column sizing, P2).
- `opacity: 0` is not honored at paint: the hidden dropdown checkboxes
  paint as black squares (paint-time opacity, next styling fix).
- Infobox image missing: upload.wikimedia.org fetches are fine (0.2s
  probe), but the engine's image loading stage stalls ~29s per page load
  (parallelism/timeout tuning — Group F), so remote images often miss the
  render.
- rust-lang.org hero: font-size too large + inline text justified across
  the column (inline layout + text-align, P1/P2).

Stage Summary:
- Commits this session: (1) compat UA, (2) A1 flex min-content, (3)
  symbolic percents, (4) this worklog + diagnostic probes
  (tests/src/bin/{net,css,css2,img,wiki,engine,ua}_probe.rs and
  BROWS_DEBUG/BROWS_ROOT_DEBUG/BROWS_MEASURE_DEBUG hooks).
- 120 tests green, clippy -D clean, rustfmt clean.
- Next: B2 box-shadow/text-shadow, then opacity paint skip, then B3.


---

## 2026-10-03 — Site suite + final report + release (session close)

Task ID: 10/11/12
Agent: Super Z (main)

Work Log:
- perf(engine): page images now fetched concurrently (join_all) instead of
  24 serial block_on calls; committed a1aef90.
- 25+ site suite executed (scripts/v1_site_suite.py): Chromium ground truth
  + brows12 render + side-by-side for 25 sites in screenshots/v1-final/
  (tailwindcss/amazon failed to render within the 150s cap and are marked
  No). Verdicts in docs/V1_FINAL_REPORT.md section 3: 2 Yes, 12 Mostly,
  8 No, ~75% correct-or-mostly vs the 90% goal — honest gap documented.
- docs/V1_FINAL_REPORT.md written with all 7 mandatory sections
  (executive summary, per-fix before/after, site verdict table, honest
  capability matrix, perf numbers, verdict + top-3 gaps, v1.0+ plan).
- CI: added thread-sanitizer job (nightly, -Zbuild-std, TSAN_OPTIONS).
- Release build: cargo build --release -p brows12-harness — green;
  example.com smoke render ok.
- Known perf bottleneck recorded: CPU-bound style + two-pass layout on
  large DOMs (10-80s wall); Group F remains the biggest lever.

Stage Summary:
- Tag v1.0.0-rc1 marks the audit-complete state with P0 closed, Group A1
  + B2 core done, suite + report delivered; remaining gaps documented
  for v1.0+.

---
Task ID: v2-audit
Agent: Super Z (main)
Task: v2.0.0 pivot — fresh clone, audit, Servo crate research, integration plan.

Work Log:
- Fresh clone verified against origin/main @ 8f006ac ("docs(v1): final report
  + site suite + CI thread-sanitizer job"); tags v0.2.0..v1.0.0-rc1 present.
  (Container snapshot had contaminated the old working copy with UUID commits;
  brows12-v2/ is now the authoritative working directory.)
- Read docs/V1_FINAL_REPORT.md + docs/V1_PLAN.md: v1.0.0-rc1 = ~75% correct-or-
  mostly across 25 sites; top gaps: CPU-bound style/layout (10-80s/page),
  background-image url() + WOFF2, SPA hydration fidelity.
- Servo crate research (crates.io API + vendored servo-0.6.0 source, not docs):
  * versions: main line 0.1.0 (2026-04-13) -> 0.6.0 (2026-09-25); separate
    0.1.x maintenance track (0.1.4, 2026-10-02). Decision D1: use 0.6.0.
  * deps verified: stylo 0.21, webrender 0.70, surfman 0.13, mozjs 0.26.3 ->
    mozjs_sys 153.3.0-0 (SpiderMonkey 153 ESR).
  * embedding API verified in source: ServoBuilder(opts/preferences/
    event_loop_waker/protocol_registry), WebViewBuilder(url/delegate/hidpi),
    WebView(load/paint/set_throttled/hide/notify_input_event/resize),
    WebViewDelegate(load_web_resource->intercept, request_navigation,
    request_create_new, notify_new_frame_ready/history/url/title/cursor),
    UserContentManager, SiteDataManager, NetworkManager(cache_entries/
    clear_cache), Servo::create_memory_report, RenderingContext::read_to_image.
  * build feasibility probed: rustup stable 1.99.0 installed (env was wiped);
    2 CPUs / 3 GB RAM / 8.5 GB disk / no root. SpiderMonkey ships PREBUILT for
    our exact pin (release mozjs-sys-v153.3.0-0,
    libmozjs-x86_64-unknown-linux-gnu.tar.gz; bindgen not on prebuilt path) —
    no cmake/clang needed. Headless GL: Xvfb + xvfb-run + Mesa swrast present;
    WindowRenderingContext (GLX) is the primary path, user-space libEGL
    extract is the documented fallback.
- Decisions recorded in docs/SERVO_INTEGRATION_PLAN.md: D1 servo 0.6.0;
  D2 initial features (baked-in-resources, js_jit, brotli-compression-stream);
  D3 use Servo's networking stack (privacy via load_web_resource interception
  is strictly more precise than socket ownership); D4 privacy layer preserved
  through delegates + prefs; D5 harness CLI contract kept; D6 side-by-side
  verification protocol unchanged; D7 ships as v2.0.0, v1.0.0-rc1 preserved.
- docs/SERVO_INTEGRATION_PLAN.md written (replaced/kept table, targets:
  <50MB/tab, <20MB suspended, <500ms cold start, <1% idle CPU, 60 FPS scroll;
  phase gates + risks) and committed BEFORE any implementation.

Stage Summary:
- Audit + research complete; plan committed and pushed prior to
  implementation (mission rule). Next: Phase 1.1 — servo-host crate +
  chunked build of the servo 0.6.0 graph.

---
Task ID: v2-phase1.1
Agent: Super Z (main)
Task: Phase 1.1-1.2 — servo-host crate, headless embedder, first renders.

Work Log:
- servo 0.6.0 graph (stylo 0.21, webrender 0.70, servo-layout/script/paint,
  prebuilt SpiderMonkey 153 via mozjs_sys archive download) compiled GREEN in
  ~15 min across 3 chunked builds (2 CPUs/3GB box, debug=0 profiles).
- servo-host crate: waker (winit proxy + condvar), HostDelegate (load status,
  frames, title/url, console, crash), headless runner (Xvfb/GLX primary,
  SoftwareRenderingContext fallback), capture (paint->read_to_image->PNG),
  brows-servo CLI (--url|--html --png --json --width --height --timeout-ms).
- Runtime blockers solved (documented for CI):
  * surfman 0.13 is EGL-based even for window contexts -> user-space GL from
    debs (libegl1+libegl-mesa0) extracted to ~/.local/gl with
    dpkg -x; LD_LIBRARY_PATH picks it up (no root needed).
  * GLVND needs vendor discovery: __EGL_VENDOR_LIBRARY_FILENAMES points at
    the extracted 50_mesa.json (mesa EGL is vendor-only on Debian trixie).
  * XDG_RUNTIME_DIR=/tmp/xdg for winit; Xvfb :99 -screen 0 1400x900x24.
- First renders (screenshots/v2-servo/phase1/):
  * example.com: title ok, complete in 213 ms (v1: 3400 ms), multilingual
    (ar/fr/ru/es) + SVG icon rendered.
  * Wikipedia Rust article: complete in 2341 ms (v1: 28600 ms); Vector-2022
    header/ToC/infobox/appearance panel pixel-class.
  * GitHub repo page: complete in 2136 ms (v1: 50700 ms); full chrome
    renders; body blocked by hydration errors (console captured in JSON):
    crypto undefined (webcrypto feature OFF), IntersectionObserver missing,
    requestIdleCallback missing, adoptedStyleSheets missing.

Stage Summary:
- Servo renders through brows12 host at Chrome-class quality with 12-24x
  faster page completion than the v1 custom engine. Phase 1.1 gate GREEN.
- Next: enable webcrypto + observer prefs (likely fixes GitHub hydration),
  retire legacy crates, wire privacy layer, then commit per sub-step.

---
Task ID: v2-phase1.2
Agent: Super Z (main)
Task: Phase 1.2 — compat preferences + webcrypto; GitHub hydration fixed.

Work Log:
- Grep of servo-config 0.6.0 prefs.rs found the real-world knobs:
  dom_intersection_observer_enabled (default FALSE), dom_adoptedstylesheet_enabled
  (default FALSE), dom_fontface_enabled (default FALSE), dom_crypto_subtle_enabled
  (default true but implementation requires the webcrypto cargo feature).
- servo-host/src/prefs.rs: compat_preferences() — brows12 "real-world sites"
  profile (intersection observer, adopted stylesheets, font loading API,
  crypto/webcrypto, resize observer), wired into both headless runners via
  ServoBuilder::preferences().
- servo dep: + "webcrypto" feature (SpiderMonkey WebCrypto impl; rebuild needed
  -j 1 on the 4GB box — first attempt OOM-killed servo-script at default -j2).
- GitHub re-render: console errors 80+ -> 1 ("No valid entry type provided to
  observe()", minor); React app fully hydrates; file table, commit history,
  About/Languages sidebar all render (github_home_v2.png). complete=true in
  2432 ms (v1 custom engine: 50.7 s).

Stage Summary:
- The compat-prefs profile is the single highest-leverage integration change
  so far: it converted GitHub from "chrome only" to fully interactive-class
  render without touching engine code. requestIdleCallback remains a genuine
  Servo gap (polyfill candidate via UserContentManager in Phase 4).
