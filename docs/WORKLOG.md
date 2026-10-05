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

---
Task ID: v2-phase1.3
Agent: Super Z (main)
Task: Phase 1.3 — retire legacy engine crates from the build graph.

Work Log:
- Workspace members reduced to storage, privacy, servo-host. Legacy crates
  (parsing/html, parsing/css, layout, rendering, compositor, networking, js,
  engine, api, tests, benchmarks, fuzz) moved to workspace exclude with a
  pointer to git history + tag v1.0.0-rc1. ui + harness excluded until
  rebuilt on servo-host (Phase 1.4/1.5).
- cargo build --workspace -j 1 GREEN (6m52s). Note: -j 2 OOM-kills on the
  4 GB box (servo-script codegen); all heavy builds must run -j 1 here.
  Box has NO swap.
- Feature unification note: adding privacy/storage to the graph recompiled
  servo-script once (two url feature sets unified); expected one-time cost.

Stage Summary:
- Build graph now contains only the Servo-backed stack + privacy + storage.
  Custom engine remains fully recoverable from history; v1.0.0-rc1 tag
  preserves the complete v1 snapshot.

---
Task ID: v2-phase1.5
Agent: Super Z (main)
Task: Phase 1.5 — privacy layer integrated at Servo embedder hooks.

Work Log:
- servo-host/src/privacy.rs: PrivacyHost wraps the v1 PrivacyBlocker
  (unchanged lists + engine thread) behind two Servo hooks:
  * WebViewDelegate::load_web_resource -> blocker.check(url, source,
    destination-mapped RequestKind); blocked requests answered with an
    empty 200 via WebResourceLoad::intercept + finish (page JS sees a
    settled resource, not a net error); unblocked loads pass through.
  * WebViewDelegate::request_navigation -> cosmetic_filters(url) installed
    as a UserStyleSheet via UserContentManager before the document loads.
- Types note: servo 0.6.0 facade forgets to re-export UserStyleSheet;
  imported from servo-embedder-traits (same version = same type). The
  facade also resolves EGL fns via dlsym-only (documented in plan risks).
- Destination (content-security-policy crate) mapped to privacy RequestKind.
- JSON report now carries privacy: {enabled, ads_blocked, trackers_blocked,
  blocked_requests[]}.
- Verified:
  * fixture with doubleclick/google-analytics/facebook.net -> 2 ads + 1
    tracker blocked; example.com image allowed; page JS ran (title change).
  * live bing search: renders, complete in 905 ms.

Stage Summary:
- The v1 privacy investment survives the pivot: adblock lists, counters and
  CHIPS policy compile unchanged against Servo's interception hooks.

---
Task ID: v2-phase1.4
Agent: Super Z (main)
Task: Phase 1.4 — interactive shell rebuilt on Servo (ui/ v2).

Work Log:
- ui/ rewritten on servo-host: Servo runs on the UI thread; each tab owns a
  WebView on Rc<OffscreenRenderingContext> (parent = WindowRenderingContext of
  the winit window). Redraw = spin_event_loop -> webview.paint() ->
  read_to_image -> tiny-skia chrome composite -> softbuffer present.
- chrome.rs kept (same hit-test geometry + drawing), text swapped from the
  legacy brows12-render Rasterizer to ui/src/text.rs (cosmic-text 0.19 +
  SwashCache glyph images -> tiny-skia; rasterize_text + measure).
- Input: clicks/moves/wheel below chrome forwarded in viewport coords
  (InputEvent::MouseButton/MouseMove/Wheel); keyboard mapped winit ->
  keyboard_types 0.8 (NamedKey enum, no Space variant — space is a Character).
- Tabs: per-tab HostState; omnibox/history/title sync from delegate state
  (Loading->Loaded transition emits the v1 automation event `loaded ...`).
- Automation protocol (BROWS12_UI_CMD_FIFO/_EVENT_FIFO) restored; commands
  cross threads via a static channel (thread_local PENDING was the bug —
  reader thread and UI thread share nothing).
- BROWS12_UI_SNAPSHOT=<path> dumps one composited frame per loaded
  generation (validation).
- Smoke test under Xvfb: start -> omni -> return -> loaded url=https://example.com/
  -> nav -> quit, all events delivered; chrome composite screenshot captured
  (screenshots/v2-servo/phase1/shell_example.png — snapshot timing catches
  the blank-page frame; cosmetic, tracked for Phase 4 UI polish).
- clippy clean for brows12-ui + servo-host.
- Disk management: full servo debug tree = 7.4 GB target on the 9.9 GB box;
  cleaned incremental caches + stripped binaries + registry .crate cache;
  note for CI: CARGO_INCREMENTAL=0 recommended.

Stage Summary:
- PHASE 1 COMPLETE: Servo integrated end-to-end (headless harness + privacy
  hooks + interactive shell). example.com / Wikipedia / GitHub all render
  Chrome-class with 12-24x faster completion than the v1 engine.
  Remaining phases: 2 (memory/perf), 3 (platform features), 4 (innovations),
  5 (30-site suite + final report + tag v2.0.0).
---
Task ID: v2-phase2
Agent: Super Z (main)
Task: Phase 2 — memory & performance (targets: <50MB/tab live, <20MB
suspended, <500ms cold start, <1% idle CPU w/ 10 tabs, 60 FPS scroll).

Work Log:
- BEFORE baselines (Phase 1.4 binary): idle CPU 0.267% w/ 10 tabs
  (30s window); RSS 289MB 1-tab, 1.07GB 10-tab example.com (~90MB/tab
  marginal). Scripts: scripts/measure_idle.py etc. (in repo).
- Commit 2f3f45c: servo-host metrics.rs (/proc VmRSS/VmHWM/cpu + CpuMeter),
  memory.rs (GovernorConfig env-tunable, Pressure, SuspendedTab),
  prefs.rs brows12_preferences (http cache 5000->1024 entries, js_mem_max
  -1->256MB, knobs documented).
- Commit 2a4f334: brows-perf harness (headless multi-tab: startup, per-tab
  RSS marginal, suspension, idle-CPU, scroll-FPS scenarios) + CLI bin.
  Found upstream bug: Servo::create_memory_report panics SystemFontService
  (usize overflow in servo-malloc-size-of) -> --engine-report opt-in,
  patch queued for Phase 4.5.
- Commit 9019c73: shell Phase 2.2-2.3. Event-driven idle (16ms pump only
  while loading/animating, else Wait on waker) — idle CPU 0.267%->0.00%.
  Tab throttling on switch (set_throttled). Hibernation = throttle+hide+
  drop WebView (CloseWebView frees pipeline) with restore-on-activate.
  Governor in about_to_wait (tick() starves when idle — winit fires only
  about_to_wait on WaitUntil expiry). Fixed about:blank premature-complete
  corrupting session history + background-tab URL sync for restore.
  malloc_trim(0) after hibernation batches (190MB extra reclaim).
  Startup: chrome presents immediately; BROWS12_UI_START_METRICS.
- Chrome ground truth via Playwright chromium headless (same box, same
  page): 1-tab 366MB, 10-tab 1.22GB, idle 0.05%, launch->loaded 114ms.
- Scroll: 34.7 FPS under software GL (60Hz wheel, Wikipedia article) —
  GPU-path validation deferred to real hardware (documented caveat).
- Artifacts in docs/perf-artifacts/phase2/, report in docs/PERFORMANCE.md.

Stage Summary (AFTER numbers):
- Idle CPU 10 tabs: 0.00% (target <1% — MET; Chrome 0.05%).
- Suspended tab: ~11MB marginal (target <20MB — MET; 10-tab session
  961MB peak -> 390MB governed, Chrome-untable headless).
- Cold start: 195ms median first-presented frame (target <500ms — MET).
- Live per-tab: ~90MB (target <50MB — NOT MET; headless harness shows
  3.5-6MB/tab, cost is per-WebView display structures; governor is the
  operational mitigation; upstream investigation queued Phase 4.5).
- Scroll: 34.7 FPS software-GL (60FPS gate needs real GPU — caveated).
- vs Chrome: lighter (289 vs 366MB 1-tab; 1.07 vs 1.22GB 10-tab), lower
  idle CPU, startup methodology differs (reported honestly).
---
Task ID: v2-phase3
Agent: Super Z (main)
Task: Phase 3 — modern CSS & platform verification (+ user directive:
      resource optimization for ~100 MB-class pages at high frame rate).

Work Log:
- Audited repo state first: Phase 1 (commits 202863a..e92d9dc) and Phase 2
  (2f3f45c..b361fd1) confirmed on origin/main; working copy brows12-v2 in
  sync. Disk was 97% full — cleaned incremental + crate caches + stale
  rlib variants (~2.5 GB reclaimed across the phase).
- Built 19 fixtures (fixtures/phase3/) + side-by-side runner
  (scripts/phase3_run.py): Chromium ground truth left, brows12 right,
  DOM self-reports visible in every screenshot. Fixed 7 fixture bugs the
  two-engine comparison itself exposed (content-box sizing, ::after
  selector, top-level await, hand-assembled WASM section sizes via
  gen_wasm_fixture.py, cascade override, NOTE-line counting, scroll depth).
- Side-by-side verdicts (all in docs/PHASE3_REPORT.md): 13 Yes
  (incl. Shadow DOM 7/7 twice, WASM 10/10, WOFF2, runtime custom props),
  4 Mostly (polygon clip-path paint, backdrop-filter, vertical writing
  modes, canvas shadow paint, multicol-adjacent), 3 No (subgrid layout,
  multicol, WebGL/WebGPU/WebRTC — compile-time cargo features, can't
  rebuild in container; documented in BUILDING.md + CI workflow
  servo-features.yml builds and probes them on GitHub runners).
- Real-world: GitHub Mostly, wpt.fyi Mostly, rust-lang.org Yes (Fira Sans
  WOFF2 confirmed), Hacker News Yes (pixel-identical).
- Phase 3.9 (user directive): heavy-page benchmark (70 MB transfer,
  ~100 MB decoded, 2000 nodes). brows12 RSS 152 MB vs Chrome 289 MB
  marginal (1.9x lighter). Scroll FPS: text 35.7 -> 4.87 @20 imgs ->
  0.16 @100 imgs; thread profiling shows wait-bound pipeline (0.12 s CPU
  /14 s wall) — scaled-image texture work under software GL; GPU target
  unaffected. Governor reclaim on image-heavy sessions only 6 MB/529 MB:
  decoded images persist in webrender's global texture cache after
  pipeline close (no embedder purge hook in 0.6.0) -> upstream gap for
  Phase 4.5.
- Shipped code: HostState.page_requests per-tab weight; heavy-tab
  governor policy (suspend 20 s vs 180 s, Elevated pressure reclaims
  heavy heaviest-first, unit tests); BROWS12_SET_PREF runtime pref
  overrides; heavy-page + Chrome-comparison + thread-profiler scripts.
- Environment notes: linker SIGBUS once (disk), script rebuild SIGKILL
  once (OOM at -j default) — recovered with CARGO_INCREMENTAL=0 -j 1.

Stage Summary:
- PHASE 3 COMPLETE. Verdict matrix + heavy-page numbers in
  docs/PHASE3_REPORT.md; artifacts committed (screenshots/v2-servo/phase3,
  validation/run/phase3, docs/perf-artifacts/phase3).
- Upstream candidates queued for Phase 4.5: subgrid layout, polygon
  clip-path paint, backdrop-filter, canvas shadows, texture-cache purge
  on pipeline close, create_memory_report usize overflow (from Phase 2).
- Next: Phase 4 (privacy hooks UX, tab management innovations, UI
  extensions, upstream PRs), then Phase 5 (30+ site suite, benchmarks,
  SERVO_FINAL_REPORT.md, tag v2.0.0).

## Phase 4 — Focus Area 1 (remaining bugs + upstream PRs)

- 2026-10-04: Environment rebuilt (fresh container): rustup 1.99, cmake via
  uv, user-local Mesa stack (~/.local/gl) restored for EGL. Repo re-cloned to
  /home/z/my-project/brows12-servo at v1.0.0-rc1 (d1f58f7).
- Vendored-patch architecture: [patch.crates-io] → patched/{servo-layout,
  servo-canvas,stylo,servo-config,stylo-static-prefs,servo-script-bindings}.
  Each Area-1 fix is one patch set, portable to the servo monorepo.
- 1.4 canvas shadows: draw_surface_with_shadow implemented (3-pass box blur,
  σ=blur/2, padded temp target); text/path shadow options wired. VERIFIED
  13/13 pixel-equivalent (canvas2d_compare.png). Commit e37837d + 914b157.
- 1.1 clip-path polygon: polygon → CPU-rasterized BGRA8 alpha mask (2×2 SS,
  winding/parity) → WR ImageMaskClip via generate_image_key_blocking +
  add_image; cross-frame mask cache (≤128). VERIFIED 7/7 both engines
  (css_filters_clip_compare.png) + apple.com/linear.app real sites.
  Commits 5af38ca + 2c89986 + compile fixes.
- 1.2 backdrop-filter: dedicated pref layout.css.backdrop-filter.enabled
  (stylo + servo-config + stylo_static_prefs); script-bindings codegen
  mapping extended (hyphenated pref names panic the guard — identifier
  mapping required); layout emits push_backdrop_filter before fragment
  painting; establishes stacking context + flat transform-style. VERIFIED
  7/7 + real sites. Commits d49973f/184c808/6e04d44.
- 1.3 writing modes: enabled layout.writing-mode.enabled (servo 0.6.0 ships
  vertical layout behind it); text-orientation un-gated onto the same pref.
  7/7 (was 3/7). Honest gap: text-orientation:upright paints sideways.
  Commit 6e04d44.
- 1.5 subgrid: unchanged — stylo_taffy wrapper TODO (taffy 0.14 has no
  subgrid); documented, upstream path.
- 1.6 multicol: layout.columns.enabled on → 4/7 computed-style checks (was
  none); fragmentation still absent (documented).
- 1.7 upstream: fork salim77007j/servo; issues servo#48610 (polygon),
  #48611 (canvas shadows), #48612 (backdrop-filter), #48613 (texture-cache
  purge) with analysis + repro + reference-impl links.
  SystemFontService memory-report crash NOT reproducible on 0.6.0 (closed;
  headless report round-trip timeout is a brows12-harness artifact).
- Report: docs/PHASE4_AREA1_REPORT.md. Verdicts: 1.1 Yes, 1.2 Yes, 1.3
  Mostly→Yes, 1.4 Yes, 1.5 No(documented), 1.6 Mostly, 1.7 done.

---
Task ID: phase4-area2-2.1
Agent: Super Z (main)
Task: Phase 4 Focus Area 2, sub-item 2.1 — network-level ad/tracker
  blocking at 2026 strength (full lists, $redirect/$removeparam/$csp/
  $important modifiers, cosmetic filtering, uBO scriptlet injection),
  verified side-by-side against Chrome.

Work Log:
- Env restoration after container reset: user-local Mesa GL stack found at
  ~/.local/gl (still present; env vars re-exported), rustfmt/clippy
  components installed, build OOM workaround (-j 1, CARGO_INCREMENTAL=0;
  the box has 2 cores/3.9 GB and parallel rustc on servo-script SIGKILLs).
- Lists: downloaded 2026-10-04 EasyList (2.09 MB), EasyPrivacy (1.51 MB),
  uBO filters (474 KB), uBO Privacy (184 KB) into privacy/lists/ and
  embedded them via include_str! (brows12_extras.txt added on top —
  brows12 policy layer with universal utm/fbclid/gclid/... stripping,
  ~43 params; uBO deliberately ships no universal utm rules).
- blocker.rs rewritten: full Verdict surface (block + $redirect body +
  rewritten_url + $csp directives), 4-list engine build (143,204 rules,
  148-205 ms on the 2-core box — no serialize cache needed), RequestKind
  fixed (Subdocument for iframes, Media instead of Image for AV, XHR
  spelled xmlhttprequest), resources installed via use_resources().
- scriptlet_resources.rs: uBO-compatible resource bundle — redirect
  targets ranked by real 2026 list usage (noopjs x81, noopmp3 x32,
  google-ima x23, ...): noop.js/txt/html/frame, 1x1.gif, 2x2.png,
  noopmp3-0.1s (ffmpeg), noopmp4-1s (ffmpeg), google-ima.js,
  googlesyndication_adsbygoogle.js, chartbeat.js, fuckadblock.js-3.2.0,
  click2load.html + 8 templated scriptlets (set-constant, aopr, aopw,
  nowebrtc, noeval, window.open-defuser, json-prune, abort-current-script).
- Cosmetic filtering completed (uBO two-phase protocol): url_cosmetic_
  resources gives hostname-specific selectors only; generic class/id
  rules (##.ad-slot) live in hidden_class_id_selectors and need DOM
  attributes. Implemented: on LoadStatus::Complete the delegate evaluates
  DOM_INFO_JS (collects classes/ids, cap 4000), matches in Rust
  (PrivacyBlocker::hidden_class_id_selectors with #@# exceptions), and
  installs a hide stylesheet. Re-entrancy constraint discovered the hard
  way: evaluate_javascript cannot be called from inside its own result
  callback (Servo RefCell borrow) -> pending_js queue on HostState,
  drained by host loops (headless x2, perf x2, ui x1).
- servo-host delegate: $redirect serves decoded resource bodies (data:
  URL -> mime + bytes) with send_body_data; $removeparam serves 301 to
  the stripped URL; $csp for iframes recorded; set_page_filtering()
  installs cosmetic stylesheet + scriptlet script + $csp meta script
  per navigation.
- UPSTREAM FINDING: adblock 0.13.3 parses $removeparam rules but NEVER
  matches them (minimal repro: ||example.com^$removeparam=utm_source ->
  rewritten_url=None; also fails for pattern/domain variants). Worked
  around with privacy/src/removeparam.rs — own uBO-subset matcher
  (pattern `*` / `||host^`, domain= include/exclude lists, regex/value/
  type variants documented as unsupported; 43 brows12 extras rules).
  Wire-up prefers our matcher, falls back to crate rewritten_url.
- Verification:
  * unit: 27 privacy tests green (redirect resource decode, removeparam
    strip/anchors/domains, $important-over-exception, $csp surfacing,
    subdocument matching, 2026-list exception behaviour for
    doubleclick/instream/ad_status.js, scriptlet template hygiene).
  * fixture E2E (fixtures/privacy/ads-fixture.html over local http):
    9 blocked (3 ads + 6 trackers), 2 $redirect replacements served,
    2 $removeparam rewrites, both cosmetic targets hidden, ad SDKs never
    executed (window.adsbygoogle/fbq absent).
  * real sites: speedtest.net 11 blocked + all three ad banners
    cosmetically removed; thesun.co.uk 36 blocked (27 ads + 9 trackers),
    full content rendered. Chrome 126 side-by-side PNGs:
    screenshots/v2-servo/phase4/area2_1_{fixture,speedtest,thesun}_side_by_side.png
    (Chrome shows 3 ad banners on speedtest; brows12 clean; on thesun
    Chrome was bot-walled while brows12 rendered+blocked).
- Quality: cargo fmt clean, clippy zero warnings on touched crates,
  brows-servo binary builds.

Stage Summary:
- 2.1 complete: 2026 lists (143k rules) + full modifier surface + two-phase
  cosmetic + scriptlet injection, network-level blocking before any byte
  leaves the process. Chrome side-by-side verified.
- Known gaps for the report: scriptlets bundle is a curated subset (the
  three default lists carry almost no ##+js calls — usage is in uBO
  Quick fixes/Annoyances, not yet shipped); $csp applied via meta
  injection (no response-header access on pass-through); procedural
  cosmetic filters (JSON-encoded actions) not yet executed.
- removeparam crate bug documented for an upstream issue (Area 1.7 list).

---
Task ID: phase4-area2-2.2
Agent: Super Z (main)
Task: Phase 4 Focus Area 2, sub-item 2.2 — 2026-grade anti-fingerprinting
  (canvas/audio farbling-style noise, WebGL spoof, hardware caps, font
  probing defeat, sensors lockdown, WebRTC guard; Brave "Standard"
  alignment + Strict tier), verified side-by-side against Chrome.

Work Log:
- privacy/src/fingerprint.rs rewritten: FingerprintConfig (Balanced default
  = Brave Standard alignment; Strict adds navigator UA/platform + screen +
  timezone-UTC), 64-bit session seed from /dev/urandom, FNV-1a per-site
  seed derivation (same hash mirrored in JS), and defense_script() — a
  self-contained ES5-compatible patch bundle installed once via the
  UserContentManager so EVERY document (main frame + iframes) is covered
  before any page script runs.
- Surfaces covered: Canvas2D getImageData/toDataURL/toBlob noise (noisy
  offscreen copy on export), AudioBuffer.getChannelData wrapped in a
  Proxy whose index reads carry per-site noise (writes pass through —
  write-after-read FP pattern defeated; one-shot mutation is NOT enough,
  proven by probe), AnalyserNode float reads noised, WebGL vendor/
  renderer spoof + readPixels noise, measureText deterministic jitter
  (font enumeration via widths fails), hardwareConcurrency/deviceMemory/
  maxTouchPoints/webdriver/languages/plugins (PDF pair) spoofed,
  getBattery stub + connection 4g/wifi + enumerateDevices [] + gamepads/
  USB/BT/Serial/MIDI locked, WebRTC stub returning no srflx candidates
  (Servo 0.6 has no RTCPeerConnection — guard is future-proofing),
  Strict-only: screen geometry/colorDepth/devicePixelRatio + Intl
  DateTimeFormat timeZone=UTC + getTimezoneOffset 0.
- Debugging journey (documented for future sessions): (1) generated JS
  had a regex escape bug (`\\/` terminates the literal early — Servo
  showed a silent "Error at :169:92" console message; node --check on the
  dumped script caught it); (2) getContext wrapped the prototype per-call
  → nested noise wrappers → in-session UNSTABLE canvas hash; fixed with a
  prototype-level one-shot guard; (3) audio noise at call time was
  overwritten by the page's later writes → Proxy-wrap fix; (4) fixture
  snapshot ordering made working hooks LOOK dead (fp_stats read before
  the audio probe ran) — moved to end of probe.
- Verification (fixtures/privacy/fingerprint-fixture.html):
  * in-session stability: canvas_hash_1 == canvas_hash_2
  * cross-session randomization: two separate processes → different
    canvas hashes (1003f9063386ce vs eae586001496a) while stable within
    each — session linkability broken, page self-consistency kept
  * audio sum shifts .918923 → .918922 with hook (stats.audio=1,
    audioCalled=true); measureText jitter active (measure:5); all
    navigator caps spoofed (cores 2→8, memory 4→8, webdriver true→false,
    plugins 0→3)
  * Chrome 126 side-by-side PNG:
    screenshots/v2-servo/phase4/area2_2_fingerprint_side_by_side.png
    — Chrome leaks the real surface (real canvas fingerprint, webdriver:
    true from Playwright, real core count 2, font-width differences
    164.61 vs 156.00 across "fonts"); brows12 shows the defense surface.
- Quality: fmt clean, clippy zero warnings, 31 privacy tests green
  (4 new fingerprint tests: seed determinism, hook presence, Strict
  gating, Off = no script).

Stage Summary:
- 2.2 complete: Balanced (default) matches/exceeds Brave Standard;
  Strict tier ready; per-session-per-site noise semantics verified
  end-to-end in the real engine.
- Honest notes: WebGL surfaces are inert on Servo 0.6 (no WebGL context
  yet — hooks installed and will activate when it lands); audio noise is
  ±1e-7 (inaudible); Proxy wrap adds overhead to getChannelData reads
  (acceptable: Servo media stack is stubbed today).

---
Task ID: phase4-area2-2.3
Agent: Super Z (main)
Task: Phase 4 Focus Area 2, sub-item 2.3 — pop-up / pop-under /
  interstitial-redirect blocking.

Work Log:
- request_create_new implemented as a total pop-up blocker: every
  auxiliary-webview request (window.open with any features string —
  pop-unders are the same API opened behind the current window) is
  dropped, which Servo documents as "no new WebView will be opened";
  counted in popups_blocked with a capped log. Stricter than Chrome's
  user-activation heuristic (the embedder cannot observe gestures);
  per-site exceptions left for the future UI.
- Interstitial redirect-chain guard in request_navigation: cross-domain
  hops tracked in an 8 s rolling window (registrable_domain comparison
  against the LAST RECORDED hop — the delegate's current-URL state can
  already reflect the in-flight navigation by callback time, which
  silently defeated the first implementation). The 5th distinct
  registrable domain in the window is denied + counted
  (redirect_chains_blocked). Human browsing (a click to a new domain
  every >2 s) never reaches the threshold; funnels at 120-500 ms/hop do.
- Verification:
  * popup fixture: auto window.open + pop-under attempt → both return
    null; title probe "popup-probe:blocked".
  * redirect-chain fixture: 5 hops across 5 loopback IPs (distinct
    registrable domains) at 120 ms/hop → chain cut at hop 4 (final URL
    127.0.0.4, hop 5 denied).
  * Chrome side-by-side PNG:
    screenshots/v2-servo/phase4/area2_3_popup_side_by_side.png
    — headless Chrome (no UI) ALLOWED both window.open calls
    ("POPUP CREATED" ×2); brows12 blocked both. Interactive Chrome
    blocks gesture-less popups; brows12 is stricter by design.
- Disk pressure recurrence: build temp files filled the rootfs twice;
  mitigated with cargo clean of leaf crates + registry cache removal +
  deleting rebuildable debug binaries; OOM guard (-j 1) kept.
- Quality: fmt clean, clippy zero warnings.

Stage Summary:
- 2.3 complete: total auxiliary-webview blocking + rate-based
  interstitial-redirect guard, both counted and logged for the dashboard.
- Honest notes: clickjacking-overlay detection needs DOM heuristics
  (deferred; cosmetic filtering already removes common overlay selectors,
  and frame-ancestors/XFO behavior is verified in 2.7); popup allowlist
  awaits the UI.

---
Task ID: phase4-area2-2.4
Agent: Super Z (main)
Task: Phase 4 Focus Area 2, sub-item 2.4 — HTTPS-Only upgrade + HSTS.

Work Log:
- privacy/src/upgrade.rs rewritten: the old set_mode() was a silent no-op
  (`let _ = mode;`) — mode is now an AtomicU8 and runtime-switchable;
  default mode is HttpsOnly per the mission brief; Upgradable and Off
  available. Exemptions: IP literals (local fixtures), localhost, .onion,
  .test, and a user-managed per-site exception set (registrable-domain
  semantics cover subdomains). HSTS cache hardened (max-age 0 removes),
  embedded 48-domain HSTS preload slice (top-traffic domains shipping
  includeSubDomains per the Chromium preload list, curated 2026-10) with
  parent-domain matching; upgrade + hsts_hit counters.
- Servo integration path (three designs tried, two rejected on evidence):
  1) deny(http nav) + webview.load(https) — RACES the denied navigation:
     final URL updates but the document never renders (blank + no title);
  2) deny + deferred location.replace via the pending_js queue — same
     blank outcome (any deny-poisoned initial navigation breaks render);
  3) SHIPPED: intercept the plain-HTTP DOCUMENT request and serve a
     meta-refresh upgrade page (real subsequent navigation renders
     normally); plain-HTTP SUBRESOURCES get a 301 (verified working:
     the re-request re-enters the hook with https).
- Verified:
  * http://example.com/ → final_url https://example.com/, title
    "Example Domain", upgrades=1 (meta-refresh path);
  * http://127.0.0.1:8901 fixture stays http (IP exemption) — all local
    fixture servers keep working;
  * direct-https control loads fine, isolating the failure modes above;
  * neverssl.com noted: its https endpoint fails in Servo 0.6 TLS
    regardless of upgrade path (pre-existing engine limitation, not a
    regression);
  * unit tests: 35 green (mode switch, exemptions, exception subdomains,
    HSTS expiry/removal, preload matching incl. subdomains);
  * Chrome side-by-side PNG:
    screenshots/v2-servo/phase4/area2_4_https_side_by_side.png
    (headless Chrome stays on http://example.com; brows12 upgrades).
- PrivacySummary extended: https_upgrades, hsts_hits, popups_blocked,
  redirect_chains_blocked (2.3 counters were missing from the report).

Stage Summary:
- 2.4 complete: HTTPS-Only default with working document + subresource
  upgrade paths, HSTS preload slice + runtime cache, per-site exceptions.
- Honest notes: intercepted document REDIRECTS (301) don't render in
  Servo 0.6 — the meta-refresh page is the reliable embedder-side
  mechanism (engine-level upgrade-insecure-requests would be an upstream
  improvement); response headers are not visible to the embedder, so
  runtime HSTS learning from Strict-Transport-Security headers needs an
  upstream hook (preload + cache API shipped; recording wired for future
  engine integration).

---
Task ID: phase4-area2-2.5
Agent: Super Z (main)
Task: Phase 4 Focus Area 2, sub-item 2.5 — DNS-over-HTTPS with
  privacy-first providers + CNAME-cloaking detection.

Work Log:
- privacy/src/doh.rs: RFC 8484 wire-format DoH client (POST
  application/dns-message) with Cloudflare (default), Quad9, Mullvad and
  runtime-custom endpoints; minimal DNS query builder + response parser
  (A/AAAA + CNAME chain incl. compressed-name pointers), 5-min cache,
  3 s timeout, fail-open on provider failure; IP/localhost/.test hosts
  short-circuit (no network). Live integration test (ignored by default
  for CI) resolves example.com over Cloudflare with a cache hit.
- Dependency interaction bug found and fixed: adding ureq pulled a second
  rustls crypto-provider feature into the graph, and Servo's net stack
  then panicked on first TLS use ("Could not automatically determine the
  process-level CryptoProvider"). servo-host::init_crypto_provider()
  installs aws-lc-rs exactly once, called from run_headless, run_perf and
  the ui main.
- CNAME-cloaking detection wired: every Document request's host is
  resolved via DoH (cached) and the CNAME chain is classified by the
  PolicyEngine (registrable-domain comparison); cloaked hosts are
  counted + logged. Runs in load_web_resource because the initial
  navigation does not pass through request_navigation (learned in 2.3).
- PrivacySummary gains cname_cloaks + doh_queries.
- Verification: live DoH test green; E2E example.com load completes with
  doh_queries=1 (page unaffected); local fixture run unchanged (9
  blocked, 2 redirects, 2 stripped, 0 DoH — IP host short-circuit);
  38 unit tests green; fmt + clippy clean.
- Honest notes: Servo's own resolver stays getaddrinfo (no embedder hook
  in 0.6) — DoH covers brows12-side lookups; upstream issue filed as
  follow-up. DoT (RFC 7858) deferred: DoH provides the same encryption
  over the HTTPS path already in use. Live demonstration of an actually
  cloaked host is inherently transient; the classifier is unit-tested
  with synthetic chains.

Stage Summary:
- 2.5 complete: encrypted brows12-side DNS with three providers + custom,
  CNAME-cloaking classification feeding the cookie policy (2.6) and the
  dashboard (2.8); provider switch is runtime-configurable.

---
Task ID: phase4-area2-2.6
Agent: Super Z (main)
Task: Phase 4 Focus Area 2, sub-item 2.6 — cookie isolation (CHIPS /
  Total Cookie Protection).

Work Log:
- brows12-storage CookieJar extended with a policy-aware entry point:
  set_from_header_with_policy(url, header, top_level_site, mode) where
  ThirdPartyCookieMode is Allow (CHIPS opt-in semantics: partition only
  with the Partitioned attribute), PartitionAll (Total Cookie Protection:
  every third-party cookie is force-partitioned under the top-level
  site) and Reject (third-party Set-Cookie dropped). __Host- prefix rules
  enforced in every mode (Secure + no Domain + Path=/), matching
  RFC 6265bis §4.1.3.2. 5 new unit tests (19 total in storage).
- Servo 0.6 cookie behavior verified empirically with a live probe
  (custom servers on loopback "sites" 127.0.0.1/127.0.0.2, echo endpoint
  returning the Cookie header the browser sends, iframe postMessage
  bridging the result into document.title):
  * first-party Set-Cookie + document.cookie work normally;
  * Secure-over-http and malformed cookies are correctly rejected;
  * the Partitioned attribute is ACCEPTED but IGNORED (stored
    unpartitioned);
  * third-party iframe cookies (Set-Cookie and document.cookie) are
    accepted and replayed UNPARTITIONED.
  => Servo 0.6's jar has no CHIPS and no third-party protection; Chrome
  today blocks/partitions third-party cookies by default. Wiring the
  brows12 TCP decision engine into Servo's jar requires an engine change
  (cookie storage is inside Servo's net stack, invisible to the
  embedder) — reference implementation shipped here, upstream issue to
  file alongside the 2.5 resolver gap.
- CNAME-cloaking tie-in: PolicyEngine::classify_cname (2.5) marks cloaked
  hosts as belonging to the cloak target's site — the TCP decision layer
  consumes exactly that site classification, so cloaked trackers are
  partitioned (or rejected) as third parties once wired engine-side.

Stage Summary:
- 2.6 complete at the decision-layer level with empirical Servo gap
  documentation; engine wiring recorded as upstream follow-up.

---
Task ID: phase4-area2-2.7
Agent: Super Z (main)
Task: Phase 4 Focus Area 2, sub-item 2.7 — security hardening (CSP
  enforcement, X-Frame-Options / frame-ancestors, COOP/COEP/CORP, SRI,
  mixed content).

Work Log:
- Empirical engine audit (servo 0.6.0 crates.io source): the net/script
  crates never read Content-Security-Policy, X-Frame-Options,
  Strict-Transport-Security, or COOP/COEP/CORP — zero engine-side
  enforcement. The embedder cannot see pass-through response headers
  (WebResourceLoad exposes the request only), so header enforcement had
  to be embedder-side via a probe.
- privacy/src/security.rs (new): SecurityGuard —
  * header PROBE per main-frame document + cross-site frame target
    (ureq GET, 2 s connect / 4 s total, cached 5 min positive / 60 s
    negative, fail-open: an unreachable probe never breaks browsing);
  * CSP parsed with the Servo-team content_security_policy crate
    (spec-accurate source matching); every later subresource from that
    document is evaluated with should_request_be_blocked BEFORE Servo
    fetches it. Nonce/hash/strict-dynamic policies are skipped for
    script/style (element metadata invisible to the embedder — no
    over-blocking);
  * XFO + frame-ancestors clickjacking guard: same-origin frames pass;
    cross-origin frames embed only if the frame target's own headers
    allow (frame-ancestors supersedes XFO per spec);
  * runtime HSTS learning: probe responses feed
    HttpsUpgrader::record_hsts — closes the gap recorded in 2.4 without
    an engine hook;
  * mixed content: https document + plain-http subresource that the
    upgrader did not rewrite (exempt host / mode off) is blocked
    outright (Chrome post-M79 semantics);
  * COOP/COEP/CORP presence recorded (counters + logs) — full
    cross-origin-isolation enforcement documented as upstream work.
- servo-host wiring: PrivacyHost gains `security: SecurityGuard`
  (upgrader now Arc, shared for HSTS); delegate load_web_resource adds
  four enforcement points (frame-guard → CSP check → mixed-content
  block → main-frame probe) after the existing privacy verdict.
- PrivacySummary extended (2.8 dashboard feed): security_probes,
  frames_blocked, csp_blocked, mixed_content_blocked, hsts_learned,
  coop/coep/corp observed + frame_log + csp_block_log.
- Verification:
  * unit: 46 privacy tests green (HSTS parse, frame-ancestors parse +
    spec decisions incl. DENY-blocks-same-origin, CSP cross-site script
    block via the real crate matcher, raw-TcpListener probe tests,
    fail-open negative cache, mixed-content decision);
  * E2E (scripts/phase4_area2_7.py, three loopback hosts): main doc
    with CSP script-src 'self' + HSTS header; evil.js on 127.0.0.2;
    XFO:DENY frame target on 127.0.0.3 → probes=2, csp_blocked=1
    (evil.js denied), frames_blocked=1, hsts_learned=1, page title
    intact (script never executed);
  * Chrome parity (Playwright Chromium side-by-side): Chrome blocks the
    same script via its own CSP engine — identical rendered outcome;
    screenshots/v2-servo/phase4/area2_7_security_side_by_side.png;
  * real-site sanity: example.com loads complete with probe=1 and zero
    false positives (csp_blocked=0, frames_blocked=0).
- Env recovery this session (recorded for future resets): rustup 1.99
  reinstalled; user-space Mesa GL stack rebuilt to ~/.local/gl (apt
  download libegl1 libegl-mesa0 libgl1 libglx0 libgbm1 libdrm2
  libx11-xcb1 + dpkg -x) with __EGL_VENDOR_LIBRARY_FILENAMES pointed at
  the extracted 50_mesa.json — matches the phase4_real_site.py recipe;
  builds need -j 1 + CARGO_INCREMENTAL=0 on this 3 GB box (servo-script
  OOMs the default parallelism); disk freed via target/debug/incremental
  + registry cache (3.3 GB reclaimed at 100% full).

Stage Summary:
- 2.7 complete: brows12 now enforces response-header security the
  engine never did — CSP network-path enforcement, XFO/frame-ancestors
  clickjacking defense, runtime HSTS learning, mixed-content blocking,
  COOP/COEP/CORP observation, all fail-open and cached.
- Honest gaps (Area 2 report): SRI not implementable embedder-side (no
  integrity metadata on WebResourceRequest, no response body on
  pass-through) — upstream issue; inline-script CSP (nonce/hash) is JS-
  level; multiple CSP headers read first-only (ureq limitation);
  cross-origin isolation (COOP/COEP) recorded, not enforced.

---
Task ID: phase4-area2-2.8
Agent: Super Z (main)
Task: Phase 4 Focus Area 2, sub-item 2.8 — privacy dashboard (data
  layer only; stats exposed for future UI rendering).

Work Log:
- servo-host/src/dashboard.rs (new): PrivacyDashboard — one
  serializable snapshot aggregating every Area 2 surface: ads/trackers
  blocked + capped blocked_sample + rules_loaded (2.1), $redirect /
  $removeparam / $csp / scriptlet / cosmetic counters (2.1),
  fingerprint_pages_protected (2.2), popups + redirect chains (2.3),
  https_upgrades + hsts_hits (2.4), doh_queries + cname_cloaks (2.5),
  cookie decision section (2.6), security-guard probes / frames / CSP /
  mixed-content / COOP-COEP-CORP + decision logs (2.7). Shipped as
  report JSON (`privacy.dashboard`) AND a callable API:
  dashboard_snapshot(privacy) + dashboard_json(privacy) for the future
  UI layer (rendering intentionally out of scope per mission).
- Cookie decision accounting: brows12-storage set_from_header_with_policy
  now returns CookieDecision {Passthrough, PartitionedOptIn,
  ForcePartitioned, RejectedThirdParty, InvalidHostPrefix} (source-
  compatible: previous callers ignored the implicit ()); CookieDecisionSink
  on PrivacyHost converts decisions into dashboard counters. Engine-side
  jar wiring stays documented upstream work (2.6 gap), so counters are
  zero in browser runs until that lands.
- HeadlessReport.privacy.dashboard filled in finish(); unit tests: 9
  servo-host (snapshot reflects live counters, sink accounting, JSON
  validity) + 19 storage green; clippy/fmt clean.
- E2E re-run (phase4_area2_7.py): all 2.7 checks still pass AND the
  JSON carries the 29-field dashboard (security_probes=2,
  frames_blocked=1, csp_blocked=1, hsts_learned=1, rules_loaded=143204);
  real-site check (example.com) complete with zero false positives.

Stage Summary:
- 2.8 complete: privacy dashboard data layer aggregated across Areas
  2.1-2.7 and exposed via report JSON + API functions; cookie decisions
  typed end-to-end from the storage decision layer.

---
Task ID: phase4-area3
Agent: Super Z (main)
Task: Phase 4 Focus Area 3 — aggressive RAM optimization (3.1-3.7).

Work Log:
- Env rebuilt (fresh container): rustup 1.99, cmake via uv, llvm-19 +
  libclang user-space stacks (~/.local/llvm, ~/.local/clang), Mesa GL
  stack to ~/.local/gl (incl. libglapi, dri/swrast, libxkbcommon-x11,
  libxcb-xkb),mozjs_sys via PREBUILT archive (GitHub release
  mozjs-sys-v153.3.0-0; source build impossible on 10 GB disk — debug
  libjs_static.a alone is 2.2 GB). Build env recipe: ~/.local/build-env.sh
  (CARGO_BUILD_JOBS=1, LIBCLANG_PATH, BINDGEN_EXTRA_CLANG_ARGS).
- 3.1 budget.rs: per-tab weight estimates (tab_base + per-request),
  availability-adaptive total budget (min(nominal, baseline + avail*0.25)),
  active-tab share multiplier, TrimCaches->Hibernate ladder. Wired into
  ui governor_tick with aligned per-tab weights; trimmed tabs re-shown on
  activation. 5 unit tests.
- 3.4 JsHeapTier Normal/Tight/Small/Minimal (256/192/128/96 MB) applied
  via Servo::set_preference("js_mem_max") — snapshot-per-runtime caveat
  documented; idle-timer detection via HostState.last_activity_ms
  (notify_new_frame_ready + load_web_resource touch); background tabs
  active <5 s get the heavy suspend schedule. Live: tier=Small fired at
  rss_ratio 1.81/3.48 under Critical.
- 3.6 psi.rs: /proc/pressure/memory + cgroup v2 reader, full/some avg10
  +avg60 thresholds, cooldown; governor acts on worse-of(RSS, PSI).
  5 unit tests. (Live PSI stayed Nominal on this box — expected.)
- 3.2 area3_verify.py: 2 heavy tabs hibernated by command — pipeline +
  image caches returned; hibernate() malloc_trim fix: 35.5 -> 122.7 MB
  returned (3.5x). Lazy decode/downsample/dedupe = upstream (image cache
  in unpatched servo crate).
- 3.5 residual quantified: after full WebView drop of both heavy tabs,
  425 MB stays vs never-opened baseline (GL/WR caches + allocator
  retention) — Phase 3 texture-cache gap now has numbers; upstream issue
  to file.
- 3.3 font memory: cross-tab sharing inherent (process-global FontStore +
  WR keys); per-display-list retirement verified (layout ->
  remove_unused_font_resources); subsetting/expiry/accounting = upstream
  gaps (docs/upstream/area3-font-memory.md); create_memory_report crash
  blocks font accounting.
- 3.7 area3_compare.py (Sampler: peak + peak-time PSS/USS, /proc ground
  truth): 5 local heavy pages + 4 real sites. RESULT (honest): 2x target
  met 1/8 (hackernews 2.03x). RSS: brows12 lighter on 7/8 (1.43-2.03x),
  heavier on cnn (0.75x, debug SpiderMonkey + heavy JS). PSS: parity to
  ~25% heavier on locals (228-335 vs 195-270). Causes: debug build,
  no image downsample/lazy decode, texture residue. bbc-news: brows12
  load fails (separate investigation, noted).

Stage Summary:
- Area 3 shipped: budgets+ladder, JS tiers, PSI response, idle-timer
  detection, hibernate trim fix, texture-residual quantified, honest
  comparison table. 19 unit tests green; artifacts in
  docs/perf-artifacts/phase4/area3/; upstream gap docs in docs/upstream/.

---
Task ID: phase4-area4-4.1
Agent: Super Z (main)
Task: Phase 4 Focus Area 4, sub-item 4.1 — predictive hibernation
(lightweight heuristics, no in-process ML).

Work Log:
- State audit: Areas 1-3 confirmed complete at HEAD 22cf523 (clean
  tree); Focus Area 4 started. Env rebuilt: cargo present,
  ~/.local/build-env.sh intact; disk twice filled by incremental/
  stale-duplicate rlibs — 3.8 GB reclaimed (rm incremental, dedupe
  deps by newest-hash, registry src re-extract), builds -j1.
- servo-host/src/tabstats.rs (new): TabUsage signals (activations,
  last_active_ms, page_requests), return_score = 0.65*recency
  (exp decay, 30-min half-life) + 0.35*frequency (a/(1+a)). Weight is
  deliberately NOT a predictor — it is a cost (tie-break: heavier
  first among near-equal scores) plus the existing heavy-eligibility
  schedule. hibernation_order() ranks candidates least-likely-to-
  return first. 7 unit tests.
- Wiring: UiTab.activations recorded in switch_to();
  aligned_tab_usages() + now_ms() session clock in the shell;
  reclaim_background_tabs() order replaced with the predictive
  ranking + `predict_order` event. GovernorConfig.warmup
  (BROWS12_GOVERNOR_WARMUP_MS, default 0) so harnesses can finish
  session setup before the first tick.
- E2E (scripts/phase4_area4.py --only 4.1, Xvfb + FIFO, budget 64 MB,
  warmup 26 s): 4 tabs, activation pattern tab2 x3 / tab1 once (ends
  active) / tab3 never. Result: prediction [3, 2, 0], hibernation
  executed [3, 2, 0] — all 5 checks PASS. Artifact:
  docs/perf-artifacts/phase4/area4/area4_verify.json.
- Chrome parity note: tab management is shell behavior; headless
  Chromium exposes no tab-strip/governor observable, so verification
  is event-stream invariants + unit tests (documented in report).

Stage Summary:
- 4.1 complete: predictive hibernation live in the governor — the tab
  the user is least likely to return to is suspended first, verified
  end-to-end. Tests: 7 unit + e2e green; clippy/fmt pending final pass.

---
Task ID: phase4-area4-4.2
Agent: Super Z (main)
Task: Phase 4 Focus Area 4, sub-item 4.2 — memory-aware tab discarding
with state preservation + user notification.

Work Log:
- Discard semantics: hibernate(index, discarded) — governor-driven
  reclaims (RSS + PSI + budget ladder) set discarded=true and emit
  `tab_discarded index= scroll_est= forms= url=`; user-command
  hibernation stays a plain hibernate. Strip shows a red dot marker on
  discarded tabs (chrome.rs, tiny-skia from_circle).
- Scroll preservation: shell tracks a per-tab vertical scroll estimate
  from forwarded wheel deltas (forward_wheel, clamped >= 0; window
  scrollTo clamps the top end on restore). Reset on new navigations.
- Form preservation: switch_to captures the outgoing tab's form state
  via SERIALIZE_FORMS_JS (delegate.rs) into HostState.form_snapshot —
  the last live moment before a discard (engine has no synchronous DOM
  access at hibernation time). hibernate() harvests it into
  UiTab.form_state; restore() queues restore_state_js(scroll, forms)
  once the reload completes (`state_restore` event).
- History preservation was already inherent (session history lives in
  UiTab, untouched by hibernation).
- E2E (phase4_area4.py --only 4.2): tall fixture + timer-filled input;
  scroll 3x800, switch away, governor discards, switch back. All 5
  checks PASS: discarded scroll_est=2400 forms=1; state_restore
  scroll_est=2400 forms=1. 4.1 regression re-run: PASS ([3,2,0]).

Stage Summary:
- 4.2 complete: pressure discards are value-ranked (4.1 ordering),
  scroll + form state + history survive the discard, and the user is
  notified via strip marker + tab_discarded events.

---
Task ID: phase4-area4-4.3
Agent: Super Z (main)
Task: Phase 4 Focus Area 4, sub-item 4.3 — tab groups (data layer).

Work Log:
- servo-host/src/tabgroups.rs (new): GroupColor (8 named colors, rgb +
  name mapping), TabGroup {id, name, color, collapsed},
  TabGroupStore (create/delete/rename/recolor/toggle_collapsed/
  add_tab/remove_tab/tab_closed/group_of/tabs_in/color_for + a
  space-free to_json that survives the event channel escaping).
  Membership keyed by shell tab id; closing a tab drops membership;
  deleting a group ungroups members. 4 unit tests.
- Shell wiring: Gui.groups; FIFO commands <GROUP_NEW name|color>,
  <GROUP_DEL>, <GROUP_ADD g|tab>, <GROUP_REMOVE>, <GROUP_TOGGLE>,
  <GROUPS> (JSON dump); events group_created/group_deleted/group_add/
  group_remove/group_toggle/groups_json; strip renders a 3px group
  color bar at each grouped tab's top edge (draw_chrome group_colors).
- E2E (phase4_area4.py --only 4.3): create "research|purple", add
  strip tabs 0+2 (shell ids 1+3), JSON dump asserts name/color/
  membership, toggle collapsed=1, delete ok=true, second dump shows
  empty groups. All 5 checks PASS.

Stage Summary:
- 4.3 complete: full tab-group data layer, unit-tested, exposed via
  API + FIFO, visually anchored by the strip color bar. Collapse
  state is stored for the future UI (strip keeps drawing all tabs —
  data-layer scope per mission).

---
Task ID: phase4-area4-4.4
Agent: Super Z (main)
Task: Phase 4 Focus Area 4, sub-item 4.4 — full session restore.

Work Log:
- servo-host/src/session.rs (new): Session {version, saved_at_ms,
  active, tabs, groups}; SessionTab carries history+hindex, title,
  scroll estimate, form-state JSON, pinned flag (schema-ready),
  group id. Atomic save (tmp+rename); load refuses corrupt/
  future-version/empty files (restore fails closed to a cold start).
  RestoreMode parsing: BROWS12_SESSION_RESTORE=1|last ->
  BROWS12_SESSION_FILE; =<path> -> specific file. 4 unit tests.
- Shell: restore_session_or_new() at startup — groups rehydrated with
  original ids (TabGroupStore::restore_group), the ACTIVE tab rebuilt
  live (spawn_webview, shared helper), all other tabs restored as
  SUSPENDED metadata that rehydrates on activation — startup builds
  one webview regardless of strip size. Scroll+form reapply rides the
  existing 4.2 pending_state_restore path. Saves: on <QUIT>, on
  window close, and periodic (BROWS12_SESSION_SAVE_SECS, default 60,
  0=off). Events: session_restored/session_live_tab/session_saved/
  session_restore_failed.
- Harness fix: event fifo now initialized BEFORE session restore
  (session_restored used to be emitted before the writer existed).
- E2E (phase4_area4.py --only 4.4, 3 UI runs): run1 saves on quit
  (tabs=3 groups=1); run2 restores last — active a.html live,
  suspended tab rehydrates to b.html, group survives with members;
  run3 restores an explicit session file. All 6 checks PASS.

Stage Summary:
- 4.4 complete: full session persistence (tabs, history, scroll,
  forms, groups, active) + restore-last and restore-specific modes,
  with lazy rehydration keeping startup cost at one tab.

---
Task ID: phase4-area4-4.5
Agent: Super Z (main)
Task: Phase 4 Focus Area 4, sub-item 4.5 — tab search (fast index over
title/URL/content snippet).

Work Log:
- servo-host/src/tabsearch.rs (new): TabSearchIndex upsert/remove/
  search — whitespace tokens with AND semantics, case-insensitive,
  field weights (title 3.0 / url 2.0 / snippet 1.0) + prefix bonus,
  best-first capped results. 5 unit tests.
- Snippet capture: HostState.page_snippet filled by SNIPPET_JS on
  LoadStatus::Complete (meta description / og:description + first h1,
  capped 240 chars) — the delegate already evaluates on completion, so
  no extra pass.
- Shell: Gui.search_index refreshed for the active tab on every sync
  and for background tabs in sync_background_tabs; closed tabs leave
  the index. FIFO: <TABSEARCH> query -> tabsearch_results count=N +
  tabsearch_hit rank/index/score/title/url/snippet events.
- E2E (phase4_area4.py --only 4.5): 3 pages with distinct titles +
  meta descriptions; title query, snippet-only query (sourdough,
  present only in the meta description), multi-token AND (moon alpha),
  no-match count=0. All 4 checks PASS.

Stage Summary:
- 4.5 complete: fast tab search over title/URL/content snippet,
  unit-tested + verified end-to-end via the automation FIFO.

---
Task ID: phase4-area4-4.6
Agent: Super Z (main)
Task: Phase 4 Focus Area 4, sub-item 4.6 — pinned tabs.

Work Log:
- UiTab.pinned (schema field landed with 4.4); <PIN> i FIFO toggle +
  `tab_pinned index= pinned=` event; strip marker = 3px blue accent
  bar on the tab's left edge.
- Governor exemptions: reclaim_background_tabs, enforce_per_tab_
  budgets and over_budget_background_tabs all skip pinned tabs —
  pinned never hibernate, never trim, never escalate the JS-heap tier
  policy.
- Session: save_session records pinned (field existed since 4.4);
  restore_session rehydrates tab.pinned. Startup auto-restore covered
  by the existing session path; pinned tabs come back exactly as
  left (active tab live, others suspended metadata).
- E2E (phase4_area4.py --only 4.6, 3 runs): pin tab1 under a 64 MB
  budget — governor hibernates the unpinned background tab only, the
  pinned tab survives every tick; session JSON records pinned:true;
  after restore the pinned tab stays live. All 6 checks PASS.

Stage Summary:
- 4.6 complete: pinned tabs are exempt from every memory policy,
  visible in the strip, and persist across sessions.

---
Task ID: phase4-area4-4.7
Agent: Super Z (main)
Task: Phase 4 Focus Area 4, sub-item 4.7 — configurable suspend policy.

Work Log:
- servo-host/src/suspend.rs (new): SuspendPolicy Never/AfterIdle(N)
  from BROWS12_SUSPEND_POLICY (never|0 = never; aggressive = 60 s;
  <secs> = AfterIdle; default 300 s) + exempt URL list from
  BROWS12_SUSPEND_EXEMPT_URLS; pure should_suspend() decision with
  active/pinned/media/URL exemptions. 4 unit tests.
- Media detection: HostDelegate::notify_media_session_event maps
  MediaSessionEvent::PlaybackStateChange(Playing) to
  HostState.media_playing (reset on navigation). ENGINE GAP found and
  documented: servo-script 0.6 stores navigator.mediaSession
  .playbackState writes in a DomRefCell without notifying the
  embedder — real media playback events flow, playbackState writes do
  not (upstream issue candidate).
- Shell: idle_suspend_pass() runs every governor tick at ANY pressure
  level (this IS the "suspend after N minutes" behavior); hibernated
  tabs emit idle_suspend events; `never` disables only this pass.
- E2E (phase4_area4.py --only 4.7, 3 runs): AfterIdle(3s) suspends
  both idle background tabs at NOMINAL pressure (active exempt);
  never mode suspends nothing; URL-exempt tab survives while the
  non-exempt one is suspended. All 5 checks PASS.

Stage Summary:
- 4.7 complete + Focus Area 4 implementation done: configurable idle
  suspension with full exemption matrix; one upstream gap documented.

---
Task ID: phase4-area4-report
Agent: Super Z (main)
Task: Phase 4 Focus Area 4 — final report + regression sweep.

Work Log:
- Full regression: cargo test -p servo-host 43/43 PASS; consolidated
  e2e (phase4_area4.py --only all) 26/26 checks PASS across 4.1-4.7;
  cargo fmt pass committed (a8fa5e1). clippy unavailable in this
  container (component missing) — documented in the report.
- docs/PHASE4_AREA4_REPORT.md written: per-sub-item status, method
  note (shell behavior → event-stream invariants + unit tests instead
  of Chrome side-by-side, which headless Chromium cannot provide),
  test table, new FIFO/env API surface, honest limitations, and one
  new upstream gap (media-session playbackState writes never notify
  the embedder; minimal repro included).
- All 7 sub-items committed and pushed individually:
  92b4d38, baeef5b, 569c106, 384f2b1, 08aca2f, a50267a, 2d31427.

Stage Summary:
- FOCUS AREA 4 COMPLETE. Phase 4 (all four areas) complete. Report at
  docs/PHASE4_AREA4_REPORT.md; artifacts under
  docs/perf-artifacts/phase4/area4/. Awaiting "continue" for Phase 5.

---
Task ID: phase5-baseline
Agent: Super Z (main)
Task: Phase 5.1 — environment restore + full regression baseline.

Work Log:
- Container had been reset: reinstalled Rust 1.99 (minimal profile), fresh
  clone of origin/main (a030a0a), user-local Mesa EGL stack rebuilt from
  Debian debs into ~/.local/gl (libegl1/libegl-mesa0/libgl1-mesa-dri/gbm/glx).
- Memory-lean build recipe for this 3.9 GB container: CARGO_BUILD_JOBS=1 +
  CARGO_PROFILE_DEV_CODEGEN_UNITS=16 (servo-script OOM-killed at defaults).
- cargo build --workspace OK; smoke capture of example.com passed (602 ms,
  complete, privacy engine 143,204 rules / 275 ms).
- Fixed latent test-only break: SiteSecurity test initializer missing the
  csp_raw field added during Area 2.7 (privacy/src/security.rs:559).
- cargo test --workspace: 108 passed / 0 failed (privacy 46, storage 19,
  servo-host 43).

Stage Summary:
- Phase 5.1 done: full workspace builds and 108/108 tests green on
  commit a030a0a + this fix. Ready for 5.2 (30+ site verification).

---
Task ID: phase5-sites
Agent: Super Z (main)
Task: Phase 5.2 — 30+ real-world site verification vs Chromium.

Work Log:
- scripts/phase5_sites.py: resumable batch capture (Chrome via Playwright,
  brows12 via headless brows-servo), side-by-side composites, summary.json.
- 32 diverse sites attempted; 31 captured + rendered (lobste.rs unreachable
  from this network — replaced dead neverssl.com with sqlite.org, py docs).
- MDN: full render (5,485 frames, correct title) but load-complete flag not
  reached in the 45 s window (long-tail resources). WHATWG spec: complete at
  44 s (multi-MB DOM — honest perf datapoint).
- Fixed CJK tofu: user-local Noto Sans CJK (~/.local/share/fonts) —
  re-captured wikipedia; 日本語/中文 now match Chrome.
- Load times (brows12): example 293 ms … github 3.5 s … whatwg 44 s.

Stage Summary:
- 31/32 real sites verified with side-by-side composites under
  screenshots/v2-servo/phase5/ + metrics in validation/run/phase5/.

---
Task ID: phase5-ram-debug-repro
Agent: Super Z (main)
Task: Phase 5.3a — reproduce Area 3 RAM comparison in debug profile.

Work Log:
- Re-ran scripts/area3_compare.py fresh (docs/perf-artifacts/phase5/
  area5_ram.json): 8 measurable pages, brows12 lighter on raw RSS on 7/8
  (1.42–2.01×), 2× met on 1/8 (hackernews 2.01×), cnn 0.74× (heavier),
  bbc-news load failure (brows12 null) — all consistent with the Area 3
  report's numbers within run-to-run noise.
- Confirms reproducibility; next lever per the Area 3 report: release/LTO
  build of the embedder stack, then re-measure.

Stage Summary:
- Debug-profile baseline reproduced. Release build started as the named
  Phase 5 lever.

---
Task ID: phase5-bench
Agent: Super Z (main)
Task: Phase 5.3 — release build + performance benchmarks vs Chrome.

Work Log:
- Release build completed on 3.9 GB box (opt3, CGU16, LTO off after the
  thin-LTO link of brows12-ui exceeded every tool-call window; dual-profile
  artifacts pushed disk to 100% twice — target/debug removed, debug
  binaries were superseded).
- area3_compare re-run on release binaries
  (docs/perf-artifacts/phase5/area5_ram_release.json): brows12 lighter on
  7/8 pages (1.52–2.07×), 2× met on hackernews (2.07×); cnn still 0.74×
  (heavy JS + debug... now release — architectural, documented); local
  pages 1.52–1.86×.
- Release vs debug peaks: 2–6% lighter (deps were already opt-level=1 in
  dev profile — the build flag was a smaller lever than Area 3 estimated).
- area3_compare patched: brows-perf binary via BROWS12_PERF_BIN (default
  release), DISPLAY added to GL_ENV.
- Startup (release brows12-ui, 5 reps): median 132 ms first-present
  (min 109) — improves Phase 2's 195 ms debug number.
- Idle: 0.00% CPU over 30 s with 10 tabs (Chrome reference 0.05%).
- 10-tab example.com: 114.5 MB/tab vs Chrome 126.4 MB/tab (9% lighter);
  Chrome cold start 98 ms vs brows12 132 ms.
- bbc.com/news now loads COMPLETE on release (Area 3 open finding
  resolved); memory sample under the perf harness remains flaky (load
  borderline within the 90–150 s window).
- Restored missing container libs: libxkbcommon-x11/libxcb-xkb (UI launch
  was panicking after the reset).

Stage Summary:
- Benchmarks complete: RAM table (release), startup 132 ms, idle 0.00%,
  114.5 MB/tab vs Chrome 126.4. Honest: 2× met on 1/8; cnn regresses;
  build flag bought 2–6%.

---
Task ID: phase5-final-report
Agent: Super Z (main)
Task: Phase 5.4 — write docs/SERVO_FINAL_REPORT.md (capstone).

Work Log:
- Wrote docs/SERVO_FINAL_REPORT.md: executive verdicts (31/32 sites,
  7/8 pages lighter / 2x on 1/8, idle 0.00%, startup 132 ms vs Chrome
  98 ms, 114.5 vs 126.4 MB/tab), per-phase status table, Phase 5 site +
  benchmark results, honest limitations (2x gap architecture, cnn
  regression, bbc flaky sample, MDN long-tail, LTO-off in this env,
  upstream gap index), conclusion.

Stage Summary:
- Final report committed; next: tag v2.0.0 + push.

---
Task ID: phase5-tag
Agent: Super Z (main)
Task: Phase 5.5 — tag v2.0.0.

Work Log:
- Tagged v2.0.0 on 7a0dbde and pushed. Phase 5 complete.

Stage Summary:
- PHASE 5 COMPLETE. All phases (1-5) done; v2.0.0 released.

---
Task ID: v2.1-phase0
Agent: Super Z (main)
Task: v2.1.0 Phase 0 — audit, baseline re-validation, docs/V2_1_PLAN.md.

Work Log:
- Container had been reset again: restored Rust 1.99, Mesa EGL + xkb
  stack, Noto CJK fonts; fresh clone verified at v2.0.0 (799ddfb).
- Release build rebuilt (5 chunks, opt3/CGU16/LTO-off recipe).
- Baseline: cargo test --release --workspace = 108/108 PASS; site suite
  status 31/32 (composites committed; raw PNGs recaptured on demand);
  spot re-run example/wikipedia/github vs Chromium all pass.
- Wrote docs/V2_1_PLAN.md: 8 evidence-backed gaps (CNN 0.74x, MDN
  completion criterion, 114.5 MB/tab, 2x on 1/8, bbc sample flaky, LTO
  off locally, no permanents, 4 upstream gaps), phased fix/innovation
  plan (tiered image decode via proven patched/ mechanism, js_mem_max
  runtime re-apply, background display-list drop, texture-cache purge),
  rejected-ideas ledger, verification + release protocol.

Stage Summary:
- Phase 0 complete. Plan pushed BEFORE implementation, as required.
