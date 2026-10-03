# SERVO_INTEGRATION_PLAN — brows12 v2.0.0

**Mission:** replace the custom rendering pipeline with the Servo engine
(published on crates.io since April 2026), then extend, optimize, and
innovate on top of it. Do NOT build from scratch; leverage Servo's mature
CSS (Stylo), layout, rendering (WebRender), and JS (SpiderMonkey) and add
brows12's own improvements on top.

Status: PLAN (committed before implementation, per mission rules).
Companions: `docs/V1_FINAL_REPORT.md` (why we pivoted), `docs/WORKLOG.md`
(per-phase log).

---

## 1. Why pivot: the v1.0.0-rc1 audit in one page

The v1 sprint shipped a fully custom engine (14-crate workspace: custom
HTML/CSS parsing, cascade, block/inline/float/grid/flex layout, display
list + raster pipeline, QuickJS-ng bindings, wgpu/CPU compositor) and
reached **~75% Chrome-equivalent rendering** across the 25-site suite
(2 Yes / 12 Mostly / 8 No+ / 3 fail). The three gaps that will not close
by incremental work on a single-threaded custom engine:

1. **CPU cost of style + two-pass layout on large DOMs** — 10–80 s wall
   per page (GitHub 50.7 s, Vercel 82.8 s, YouTube 78.0 s). This is an
   architectural property of our engine, not a bug list.
2. **background-image url() layers + WOFF2** — the most visible class of
   visual delta on marketing pages (stripe, github, cloudflare).
3. **SPA hydration fidelity** — crates.io / vuejs.org / duckduckgo render
   shells only; they depend on deep DOM API coverage that QuickJS bindings
   would take months to complete.

All three are table stakes in a mature engine. Servo ships all three
today (Stylo + WebRender + 15 years of DOM work + SpiderMonkey), is
written in Rust, embeddable as a crate, and aligned with our
privacy/low-memory goals. Replacing the engine core is the honest move;
keeping it is a multi-year detour.

## 2. Servo crate research (done 2026-10-04)

### 2.1 Version landscape (crates.io `servo`)

| Line | Versions | Latest | Note |
|------|----------|--------|------|
| main | 0.1.0 (2026-04-13) → 0.2 → 0.3 → 0.4 (Jul) → 0.5 (Aug) → **0.6.0 (2026-09-25)** | 0.6.0 | fully modularized: `servo-layout`, `servo-script`, `servo-net`, `servo-paint`, `servo-media`, `servo-constellation`, … all pinned `=0.6.0` |
| 0.1.x maintenance | 0.1.1 → 0.1.4 (2026-10-02) | 0.1.4 | backport branch of the 0.1 API (72 deps, older ipc-channel) |

**Decision D1:** integrate **servo 0.6.0** — newest semver line, most
features, and the line the modular sub-crates are pinned to. The 0.1.4
track is a maintenance branch of an older API; adopting it would strand
us a year behind.

### 2.2 What 0.6.0 gives us (verified from the crate source, not guesswork)

- **CSS engine:** Stylo 0.21 (`stylo`, `stylo_traits`) — the same style
  system Firefox ships: full cascade, custom properties, calc()/clamp(),
  grid, flex, selectors 4, writing modes, transforms.
- **Layout:** `servo-layout 0.6.0` — parallel layout (rayon), 2025-2026
  grid/flex/flow work.
- **Rendering:** WebRender 0.70 + `servo-paint` — batched GPU rendering,
  `surfman 0.13` contexts, `SoftwareRenderingContext`,
  `OffscreenRenderingContext`, `WindowRenderingContext`.
- **JS:** SpiderMonkey **153 ESR** via `mozjs 0.26.3` / `mozjs_sys
  153.3.0-0` — JIT on by default (`js_jit`), full ES2025+ surface,
  Shadow DOM enabled by default.
- **Embedding API (the exact surface we will use):**
  - `ServoBuilder::new().opts()/preferences()/event_loop_waker()/protocol_registry().build()` → `Servo`
  - `Servo::spin_event_loop()`, `set_preference()`, `create_memory_report()` (memory governor hook), `site_data_manager()` (cookies/storage controls), `network_manager()` (cache inspection/clear)
  - `WebViewBuilder::new(&servo, rendering_context).url().delegate().hidpi_scale_factor().build()` → `WebView`
  - `WebView`: `load/load_request/reload/go_back/go_forward`,
    `notify_input_event/notify_scroll_event`, `resize`, `paint`,
    `set_throttled(bool)` (background-tab throttling), `hide()/show()`,
    `set_page_zoom()`, `focus()/blur()`, `cursor()`, `page_title()`,
    `favicon()`, `load_status()`, `url()`
  - `WebViewDelegate` (default-implemented trait): `notify_new_frame_ready`,
    `notify_url_changed`, `notify_page_title_changed`,
    `notify_history_changed`, `notify_load_status_changed`,
    `notify_cursor_changed`, `notify_crashed`, `request_navigation`
    (allow/deny), `request_create_new` (popup blocking),
    **`load_web_resource` → `WebResourceLoad::intercept(response)`**
    (request/response interception — the privacy-layer hook),
    `show_console_message`, `notify_animating_changed`
  - `UserContentManager` + `UserScript` (script injection),
    `ProtocolRegistry` (custom schemes), `ClipboardDelegate`
- **Pixel readback:** `RenderingContext::read_to_image(DeviceIntRect) ->
  Option<RgbaImage>` — first-class, verified in `servo-paint-api
  0.6.0/rendering_context.rs`.
- **Feature flags relevant to us:** `js_jit` (default), `brotli-compression-stream`
  (→ WOFF2 fetch/decode path), `webgl`, `webgpu`, `webxr`,
  `media-gstreamer` (WebRTC/`getUserMedia` media backend), `vello`
  (CPU paint backend), `background_hang_monitor`, `tracing`,
  `default_web_features` (brotli + clipboard + webcrypto + webgl + webgpu + webxr bundle).

### 2.3 Build feasibility on this container (probed, with receipts)

Constraints found: **2 CPUs, 3 GB RAM, 8.5 GB free disk, no root, no
apt install rights.** Toolchain: rustup stable 1.99.0 installed; gcc/g++,
make, python3, perl, pkg-config, autoconf present; cmake/clang/libtool/
yasm/nasm MISSING.

| Risk | Finding | Mitigation |
|------|---------|------------|
| SpiderMonkey from source (the classic blocker) | **Not needed.** `mozjs_sys` build.rs downloads a prebuilt static archive from `servo/mozjs` GitHub releases when `intl` + `jit` features are on; verified release `mozjs-sys-v153.3.0-0` (our exact pin) ships `libmozjs-x86_64-unknown-linux-gnu.tar.gz`. Bindings ship inside the archive → bindgen/libclang NOT required on the prebuilt path. | Keep `intl`+`jit` on (default). Fallback env `MOZJS_ARCHIVE=<local tarball>` if the GitHub download 404s in CI. |
| GL context for headless render | No libEGL/OSMesa installed, **but Xvfb + xvfb-run + Mesa `swrast_dri.so` + libGL (GLVND) are present** — the exact setup Servo CI uses. | Run headless renders under `xvfb-run` with `LIBGL_ALWAYS_SOFTWARE=1` using `WindowRenderingContext` (GLX). Backup: user-space `dpkg -x` extract of `libegl1`/`libegl-mesa0` into `~/.local/lib` + `LD_LIBRARY_PATH` for `SoftwareRenderingContext` (surfman EGL surfaceless). |
| Link-time RAM on 3 GB | servo-script/servo-layout are giant crates. | `debug = 0` + `strip = "debuginfo"` profiles (brows12 already sets this for workspace; replicate for servo members), default lld linker (rust ≥1.90 on x86_64-linux), build with `cargo build -p <member>` in chunked stages, never `--all-targets`. |
| Disk (8.5 GB) | Full servo dev tree with debug=0 ≈ 4–6 GB target dir. | Single shared `CARGO_TARGET_DIR`, no test binaries until needed, monitor `df` each stage; `cargo clean -p` completed giants if needed. |
| No root | Xvfb already installed; mozjs prebuilt; no cmake needed (nothing on our path builds via cmake with prebuilt mozjs; if a transitive dep demands cmake, use the portable cmake single-binary into `~/.local/bin`). | Documented fallbacks, not assumptions. |

**Decision D2 (features):** Phase 1 links
`servo = { version = "0.6.0", default-features = false, features =
["baked-in-resources", "js_jit", "brotli-compression-stream"] }`
(no clipboard → drops arboard/X11 dep; no webgl/webgpu/gstreamer until
Phase 3 verifies their runtime stacks). Widen in Phase 3 after each
feature's runtime is probed. `clipboard` re-enabled for the interactive
shell once the shell runs (Phase 1.2).

## 3. Architecture: what is replaced, what is kept, what is added

```
brows12 v2 (Servo-backed)
├── ui/            KEEP+EXTEND   winit + softbuffer shell, address bar,
│                  tab bar, bookmarks/history/downloads/settings,
│                  privacy dashboard. Drives Servo WebViews.
├── servo-host/    NEW           The embedder: ServoBuilder/WebView wiring,
│                  tab model, delegates, event-loop glue, input routing,
│                  pixel present into the shell, headless render mode.
├── privacy/       KEEP          Adblock/tracker lists (engine-agnostic),
│                  CHIPS, anti-fingerprinting prefs/policies.
│                  Integrated via WebViewDelegate::load_web_resource
│                  interception + request_navigation + UserContentManager.
├── harness/       KEEP (CLI)    `brows render --url|--html --png --json`
│                  reimplemented over servo-host headless mode.
│                  Legacy engine path retired after parity gate.
├── parsing/layout/rendering/compositor/js/engine/api
│                  RETIRE        Deleted from the build graph in Phase 1.3
│                  (kept in git history; tag v1.0.0-rc1 preserved).
│                  parsing/html (html5ever-based) retired with them;
│                  Servo brings its own parser.
├── networking/    RETIRE        hyper/quinn/rustls client kept only where
│                  Servo cannot do the job (none identified today —
│                  Decision D3 below).
└── storage/       RETIRE        Servo storage + SiteDataManager replaces it;
                  brows12 privacy policy wraps it.
```

**Decision D3 (networking): use Servo's stack (`servo-net`, hyper/rustls).
** Rationale: the fetch spec (CORS, mixed content, redirects, caching,
cookies, service-adjacent semantics) is deeply entangled with the
constellation; re-feeding our own responses through
`WebViewDelegate::load_web_resource` would mean re-implementing fetch to
avoid double-fetch and would strand caching/cookies. Our networking crate
had no feature Servo's lacks for browsing (our DoH and QUIC experiments
are documented in `docs/PRIVACY.md`; if a gap appears in Phase 3+ we can
route selected loads through `load_web_resource` interception — that hook
gives us a full request/response override without replacing the stack).
The privacy layer does NOT need to own the socket: blocking happens at
the intercepted-resource level, which is strictly more precise
(post-CORS, per-frame, per-origin metadata included).

**Decision D4 (privacy layer — preserve, do not lose):**
1. Adblock/tracker lists (`privacy` crate data + matcher) → run inside a
   `WebViewDelegate::load_web_resource` interceptor: build
   `WebResourceResponse` deny (or empty body) for blocked URLs, pass
   through everything else. Per-tab enable/disable.
2. Third-party cookie policy / CHIPS → `Preferences` + `SiteDataManager`
   policies; our CHIPS semantics preserved as embedder-side policy checks
   in `load_web_resource` (Set-Cookie inspection) where prefs are
   insufficient.
3. Anti-fingerprinting → Servo prefs surface (`set_preference`) for the
   knobs Servo exposes (canvas/webgl noise, reduced UA/platform info);
   anything not pref-exposed is documented as a gap, not faked.
4. Popup/new-window blocking → `request_create_new` + `request_navigation`
   delegates.

**Decision D5 (headless + harness):** keep the exact `brows` CLI
contract. `brows render` = servo-host headless: Xvfb + GLX swrast,
`WindowRenderingContext`, `webview.paint()` on `notify_new_frame_ready`,
then `read_to_image` → PNG. JSON report gains engine identity
(`"engine": "servo 0.6.0 (stylo 0.21, webrender 0.70, sm 153)"`).

**Decision D6 (verification protocol — unchanged from v1):** every
feature/site fix produces a side-by-side screenshot (brows12 | Chrome
via Playwright) before "success" may be claimed. Screenshots land in
`screenshots/v2-servo/` (Phase 1–4) and `screenshots/v2-final/`
(Phase 5 suite). Verdicts: Yes / Mostly / No.

**Decision D7 (versioning):** this integration ships as **v2.0.0**;
`v1.0.0-rc1` tag stays as the custom-engine snapshot. Legacy crates are
deleted from the workspace tree in Phase 1.3 (recoverable from git).

## 4. Phase gates, targets, and evidence

### Phase 1 — Integration (foundation)
- 1.1 `servo-host` crate with servo 0.6.0 dep; compile the full graph
  (chunked, debug=0). **Gate:** `cargo build -p servo-host` green.
- 1.2 Shell on Servo: ui window paints WebView pixels via
  `read_to_image`/present path; address bar → `webview.load()`;
  back/forward via delegate history; input routed to `notify_input_event`.
- 1.3 Retire legacy engine crates from the workspace; `cargo build`
  (whole workspace) green; README + docs updated.
- 1.4 Headless harness over Servo (`brows render`).
- 1.5 Privacy layer wired through `load_web_resource` interception with a
  unit-testable fake request + a live block demo (ad-heavy page shows
  blocked-request count in JSON report).
- **VERIFY (mission gate):** example.com, Wikipedia (Rust article),
  GitHub rendered by Servo, side-by-side vs Chrome in
  `screenshots/v2-servo/phase1/`. Commit + push per sub-step.

### Phase 2 — Memory & performance
Targets (measure with `/proc/self/status`, `perf`, and Servo's
`create_memory_report`):
- **RAM:** < 50 MB per typical page tab; < 20 MB suspended tab;
  document vs Chrome same-page numbers.
- **Startup:** cold start < 500 ms (goal; honest delta reported).
- **Idle CPU:** ~0% for 10 background tabs — implement tab suspension
  (`set_throttled(true)` + `hide()`), stop painting hidden tabs (no
  `paint()` call when hidden), background-tab timer throttling is
  engine-side (verify + report).
- **Scroll:** 60 FPS target on content-heavy pages via WebRender
  compositing; measure `notify_scroll_event` → frame latency under
  Xvfb (documented caveat: software GL here, real GPU on desktop).
- Cache/image/font eviction: tune via `NetworkManager` cache controls +
  `create_memory_report` feedback loop; document every knob.
Commit + push each sub-step with numbers in `docs/WORKLOG.md`.

### Phase 3 — Modern CSS & platform verification
Verify-and-extend list (each item: Chrome ground truth → brows12 render →
side-by-side → verdict): Shadow DOM (YouTube/GitHub/wpt fixtures),
CSS Grid advanced (template-areas, auto-fill/fit, minmax, subgrid),
sticky/fixed, transforms/transitions/animations, box-shadow/text-shadow/
border-radius, background-image (url, gradients, layers, cover/contain),
calc()/min()/max()/clamp(), custom properties (runtime updates), filters,
clip-path, writing modes, columns, web fonts incl. **WOFF2**
(rust-lang.org, GitHub, Wikipedia), Canvas 2D, **WebGL2** (three.js),
**WebGPU** (webgpu.github.io samples), **WebRTC** (enable
`media-gstreamer` + gstreamer runtime probe; demo page with
RTCPeerConnection + data channel), **WASM** (SpiderMonkey native).
Feature gaps → local patches documented + upstream PRs filed (Phase 4.5).

### Phase 4 — Innovation
1. Privacy: engine-level fingerprinting resistance defaults, privacy
   dashboard (blocked-request counts, per-site storage view via
   SiteDataManager).
2. Tabs: suspension/hibernation/restoration, memory-aware discarding via
   memory-report feedback.
3. UI: bookmarks, history, downloads, settings, privacy dashboard on the
   existing winit shell.
4. Performance: per-stage tracing (`tracing` feature), lazy paint culling
   experiments, image-decode budget experiments.
5. Upstream: every local patch to Servo becomes an upstream PR + a line
   in `docs/SERVO_UPSTREAM.md`.

### Phase 5 — Final validation
- 30+ site suite (v1 25 + Figma, Google Maps, a WebRTC demo, + 2) →
  `screenshots/v2-final/` + verdict table.
- Benchmarks vs Chrome: cold start, RAM/tab (10 tabs), idle CPU, page
  load (10 sites), scroll FPS.
- `docs/SERVO_FINAL_REPORT.md`: replaced/kept/added, perf numbers,
  feature matrix, site verdicts, honest gaps, final Yes/Mostly/No
  verdict. Push. **Tag v2.0.0.**

## 5. Memory & performance targets (mission numbers, restated)

| Metric | Target | How measured |
|--------|--------|--------------|
| per-tab RAM (typical page) | < 50 MB | `create_memory_report` + VmRSS delta |
| suspended tab RAM | < 20 MB | after `set_throttled` + drop caches |
| cold start | < 500 ms | time-to-first-frame, `perf` |
| idle CPU, 10 tabs | < 1% | /proc/stat sampling over 60 s |
| scroll | 60 FPS | frame timestamps under load |

## 6. Feature coverage targets

Shadow DOM (default-on in Servo since 2025), CSS Grid (incl. subgrid
verification), WebRTC via servo-media-webrtc/GStreamer, WebGPU,
WebGL 2, WASM, WOFF2 (brotli), Canvas 2D, full modern CSS list in
Phase 3. Feature matrix in the final report marks each
Yes / Partial / No with evidence links.

## 7. Risks (explicit, with fallbacks)

1. **Link OOM on 3 GB** → chunked builds, lld, debug=0, `codegen-units`
   bumped for the giant crates if needed (accepting slower runtime for
   buildability), swap file if permitted (`fallocate` in /home/z —
   will test; needs root, likely unavailable → then strictly chunk).
2. **Xvfb/GLX quirk** → user-space libEGL extract; worst case vello CPU
   paint feature probe.
3. **servo crate API drift between 0.5/0.6** → we pin =0.6.0 and read the
   vendored source (already downloaded) as ground truth.
4. **GStreamer runtime absent** (WebRTC) → feature-gate it out of the
   default build; document enablement steps; verify on a machine with
   GStreamer; honest gap in the report if impossible here.
5. **CI (GitHub runners) has 4 GB RAM for Linux** → CI builds servo with
   the same chunked profile flags; prebuilt mozjs keeps CI feasible.

## 8. Immediate next actions (Phase 1.1)

1. `cargo new servo-host` in workspace; add servo 0.6.0 (D2 features).
2. Chunked `cargo build -p servo-host` (observe mozjs prebuilt download).
3. Compile-fix loop against the real API surface (vendored source as
   reference), keeping `docs/WORKLOG.md` updated per sub-step.
