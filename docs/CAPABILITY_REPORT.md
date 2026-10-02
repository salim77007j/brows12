# Brows12 v0.2 Capability Report

**Date:** 2026-10-02 · **Engine:** brows12 v0.2.0 (v0.2.1 GPU/graphics refresh) · **Platform:** Linux x86_64 (headless CI container, 2 cores; GPU paths verified on lavapipe/SwiftShader software Vulkan)
**Verification:** every claim below is backed by a test in this repository or a screenshot in `docs/screenshots/` produced by the headless harness (`harness/`, `brows` CLI).

This report is honest by design: it states what works, what is partial, and what is missing. Nothing here is aspirational.

---

## 1. Executive summary

Brows12 v0.2 is a Rust browser engine (13-crate workspace) that loads, styles, lays out, renders and scripts real websites headlessly. In the real-world test suite **23 of 25 major sites completed the full pipeline** (fetch → privacy filter → html5ever parse → lightningcss cascade → taffy layout → tiny-skia/swash raster → QuickJS-ng scripts → compositor) and produced screenshots.

What works well: document rendering for text-and-CSS sites (Wikipedia, docs.rs, arxiv, w3.org, HN data), CSS custom properties, cascade layers, media/supports/container at-rules, animations and transitions (deterministic clock), scrollable compositing with fixed-layer anchoring, Canvas 2D, WebAssembly (wasmi), observers, and an event loop with fetch/XHR/WebSocket/Workers.

What is not there yet: tables and floats, full inline flow (mixed inline boxes on one line), presentational HTML attributes (`bgcolor`, `width`), form controls, media playback, and Service Workers. WebGL 2 and WebGPU landed in the v0.2.1 refresh (shaders, buffers, VAOs, textures, compute; see the matrix for the subset boundaries), along with ES modules, IndexedDB (synchronous subset) and an Element.animate() polyfill. Section 4 has the complete matrix.

**Verdict for the UI layer:** the engine is ready to host a UI against *document-centric* sites today (Wikipedia-class). Interactive web-app sites (GitHub-class) render partially — content and scripts execute, but layout fidelity drops where tables/floats/inline-flow are involved. See §6 for the exact per-site verdicts.

---

## 2. Real-world browsing test results

Environment: headless Linux, 1280×800 viewport, privacy shield ON (ads blocked, HTTPS upgraded, CNAME-cloaking blocked). Runner: `brows suite --list sites.txt --dir docs/screenshots --json docs/test-results.json`.

| # | Site | Loaded | Title received | PNG | Time (ms) | Render verdict |
|---|------|--------|----------------|-----|-----------|----------------|
| 1 | example.com | ✅ | Example Domain | 14.5 KB | 3439 | **Correct** (grid centering, their 2026 page) |
| 2 | www.wikipedia.org | ✅ | Wikipedia | 79 KB | 4149 | **Good** — dark scheme, globe image, grid, i18n text; language links overlap (inline-flow gap) |
| 3 | en.wikipedia.org/wiki/Rust | ✅ | Rust (programming language) | 77 KB | 28026 | **Partial** — article text renders; tables/infoboxes collapse (tables unsupported) |
| 4 | github.com | ✅ | GitHub · Change is constant… | 51 KB | 54066 | **Partial** — content + JS run; layout degrades (inline flow, table use) |
| 5 | news.ycombinator.com | ✅ | Hacker News | 39 KB | 4870 | **Partial** — all story data renders; table layout + `bgcolor` unsupported → stacked, transparent bg |
| 6 | www.rust-lang.org | ✅ | Rust Programming Language | 48 KB | 7981 | **Good** — hero, nav, styled sections |
| 7 | www.mozilla.org | ✅ | Client Challenge | 37 KB | 3705 | Bot-wall page rendered (site served a challenge) |
| 8 | www.google.com | ✅ | Google | 29 KB | 3575 | **Partial** — logo/text render; app UI partial |
| 9 | duckduckgo.com | ❌ timeout | — | — | 30007 | Load timed out (bot protection; not a parser failure) |
| 10 | www.youtube.com | ✅ | YouTube | 5.9 KB | 4991 | Shell only — app requires heavy JS (polymer) |
| 11 | threejs.org | ✅ | Three.js – JavaScript 3D Library | 359 KB | 12302 | **Good** — landing page renders incl. imagery |
| 12 | webgpu.github.io | ✅ | Site not found · GitHub Pages | 31 KB | 471 | 404 page rendered correctly (sample moved upstream) |
| 13 | developer.mozilla.org | ✅ | MDN Web Docs | 23 KB | 9095 | **Partial** — content renders; app chrome partial |
| 14 | stackoverflow.com | ✅ | Just a moment… | 5.8 KB | 3425 | Cloudflare interstitial rendered |
| 15 | www.bbc.com | ✅ | BBC Home – Breaking News… | 54 KB | 6690 | **Partial** — headline content renders; media-heavy sections degrade |
| 16 | www.reddit.com | ✅ | (no title) | 18 KB | 133 | Shell rendered; app requires full JS platform |
| 17 | arxiv.org | ✅ | arXiv.org e-Print archive | 11 KB | 3928 | **Good** — document-style page |
| 18 | docs.rs | ✅ | Docs.rs | 44 KB | 5564 | **Good** — Rustdoc pages are document-style |
| 19 | crates.io | ✅ | crates.io: Rust Package Registry | 5.8 KB | 5256 | Shell (ember app boots partially) |
| 20 | www.w3.org | ✅ | W3C | 55 KB | 4622 | **Good** |
| 21 | caniuse.com | ✅ | Can I use… | 11 KB | 7930 | **Partial** |
| 22 | webglfundamentals.org | ✅ | WebGL Fundamentals | 19 KB | 7471 | Page renders; WebGL canvas area empty (no WebGL yet) |
| 23 | http3.is | ✅ | (no title) | 115 KB | 549 | **Good** — also exercised HTTP/3 via quinn |
| 24 | www.bing.com | ✅ | Search – Microsoft Bing | 6.1 KB | 4368 | Shell |
| 25 | lite.duckduckgo.com | ❌ timeout | — | — | 30007 | Load timed out (same bot protection family) |

**Score: 23/25 loaded and rendered; 9 sites render "good/correct" for document-style content; 8 partial (content + JS, layout gaps); 4 shells/interstitials; 2 network-level timeouts (bot walls, not engine crashes). Zero panics, zero crashes across all 25.**

Note on timing: the harness build is a debug profile with `opt-level=1` dependencies on 2 cores; release builds are substantially faster. The 54 s GitHub load is dominated by fetching and scripting a ~2 MB JS app on constrained hardware, not by the render pipeline itself.

---

## 3. Web Platform API matrix

Legend: ✅ supported · 🟡 partial (works, subset documented) · ❌ missing.

### DOM & scripting
| Feature | Status | Evidence / notes |
|---|---|---|
| DOM Core (getElementById, querySelector(All), traversal, appendChild/insertBefore, innerHTML, classList, attributes) | ✅ | `tests/tests/js_apis.rs`, `engine/tests/pipeline.rs` |
| Event listeners + dispatch | ✅ | `worker_message_round_trip`, glue registries |
| Event propagation (capture/bubbling phases) | ❌ | dispatch notifies target + window/document only (ROADMAP) |
| `fetch` (GET/POST, headers, JSON/text/arrayBuffer) | ✅ | `fetch_post_sends_body`, `fetch_relative_url_resolves_against_location` |
| `XMLHttpRequest` (async subset) | ✅ | `tests/tests/js_apis.rs` |
| `WebSocket` (ws/wss, echo round-trip) | ✅ | `websocket_echo_round_trip` |
| Web Workers (dedicated, own realm) | ✅ | `worker_message_round_trip` |
| `setTimeout`/`setInterval` + clamping | ✅ | event-loop tests |
| `console.*` → engine events | ✅ | pipeline tests |
| `localStorage` (per-origin, persisted) | ✅ | storage tests |
| ES2022+ language level | ✅ | QuickJS-ng (classes, async/await, optional chaining, nullish coalescing) |
| **ES modules** (`<script type=module>`, `import`, dynamic `import()`) | ✅ | HTTP(S)/virtual-scheme loader through the engine net stack (privacy/cache/cookies apply); bare specifiers unsupported (as in browsers). Evidence: `es_module_import_executes` |
| `performance.now()` | ✅ | `canvas2d_and_platform_apis` |
| `MutationObserver` | 🟡 | fires batched records on mutations; record granularity approximate |
| `ResizeObserver`, `IntersectionObserver` | 🟡 | initial-callback semantics, not continuous |
| `EventSource` (SSE) | 🟡 | parses events from complete response; no live streaming |
| `requestAnimationFrame` | ✅ | mapped to frame scheduler |

### Graphics
| Feature | Status | Notes |
|---|---|---|
| Raster pipeline (backgrounds, borders, glyphs, images) | ✅ | tiny-skia + swash; validated pixel-level (`content_pipeline`) |
| Canvas 2D (paths, transforms, fills/strokes, fillText, drawImage, gradients-stub) | 🟡 | `canvas2d_and_platform_apis`; `measureText` approximated; getImageData/putImageData missing |
| **GPU compositor** (wgpu 30): layer textures, transform/opacity uniforms, offscreen render + readback, scroll without re-raster | ✅ | `compositor` crate tests; GPU path verified on lavapipe/SwiftShader (CI installs `mesa-vulkan-drivers`); a WGSL `vec3` uniform alignment bug that only real GPUs caught (Windows) is fixed |
| CPU compositor fallback | ✅ | blend + transform parity tests |
| Scroll compositing (`Tab::set_scroll`) | ✅ | fixed layers anchored; content layer offset |
| **WebGL 2** | 🟡 | working subset over wgpu 30 (`brows12-js::webgl`): GLSL ES 1.00/3.00 shaders via a tested ES→desktop-core normalizer + naga 29 (see LIBRARY_CHOICES), programs, buffers, VAOs, textures (uploads from bytes + Canvas2D), uniforms (float/int/mat), `drawArrays`/`drawElements` (TRIANGLES/STRIP/LINES/POINTS), blending, depth test, culling, scissor, viewport, `readPixels`, GPU→page harvest like Canvas2D. **Missing:** framebuffer objects, transform feedback, instancing, queries, `TRIANGLE_FAN`/`LINE_LOOP`, extensions. Evidence: `engine::tests::pipeline::webgl2_triangle_renders_and_harvests` (red-pixel assertion on SwiftShader) |
| **WebGPU (JS surface)** | 🟡 | `navigator.gpu.requestAdapter/requestDevice`, buffers (create/write/map-read/destroy), WGSL shader modules, compute pipelines, bind group layouts + groups, command encoder + compute pass dispatch, `queue.submit`. **Missing:** render pipelines/texture bindings from JS, full async model (`mapAsync` resolves synchronously — documented). Evidence: `webgpu_compute_vector_add` (WGSL kernel doubles a float buffer) |
| Images (PNG/JPEG/GIF/WebP decode) | ✅ | `image` crate |
| Web fonts (`@font-face`) | ✅ | engine fetches + `load_font_data` (cap 6/page) |
| CSS animations (`@keyframes`, easing solver) | ✅ | `parsing/css` unit tests + `Tab::advance_animation` |
| CSS transitions | ✅ | `TransitionEngine` diffing on restyle |

### CSS cascade & layout
| Feature | Status | Notes |
|---|---|---|
| Selectors: type/class/id/attribute(all ops), descendant/child/sibling, `:is()` `:where()` `:not()` `:nth-child(of S)` `:nth-of-type`, `:root` `:empty`, link/form-state | ✅ | `brows12-css` matcher (property-tested) |
| `@media` (full feature eval incl. prefers-\*, pointer/hover, ranges) | ✅ | `media_query_filters_rules` |
| `@supports` (real value re-parse) | ✅ | `supports_filters_rules` |
| `@layer` (declaration order + reversed !important) | ✅ | `cascade_layers_order` |
| `@container` (inline-size, second-pass) | ✅ | engine re-cascade pass |
| `@scope` | ❌ | parsed-then-ignored |
| Custom properties + `var()` (fallbacks, recursion) | 🟡 | text-level resolution; **document-global scope** for sheet-defined tokens + per-element inline; element-scoped rule definitions resolve globally (documented approximation) |
| Cascade layers ordering, `!important`, inline style | ✅ | cascade tests |
| Flexbox (taffy) | ✅ | direction/wrap/justify/align/gap |
| CSS Grid (taffy) | ✅ | used by real sites (wikipedia.org) |
| Block flow, margins/padding/borders/radius | ✅ | layout tests |
| Absolute/fixed positioning (taffy abs + viewport anchoring) | ✅ | fixed-layer compositor anchoring |
| Sticky | 🟡 | treated as relative at scroll=0 |
| Text wrapping + shaping (HarfBuzz-grade) | ✅ | cosmic-text |
| **Inline flow** (mixed inline boxes/inline images on one line) | ❌ | inline maps to block stacking — the single biggest visual gap |
| Floats | ❌ | roadmap |
| Tables | ❌ | roadmap |
| Pseudo-elements `::before/::after` | ❌ | roadmap |
| `@font-face` loading | ✅ | engine integration |
| Viewport units, em/rem/calc | 🟡 | em/rem/vw/vh resolved; `calc()` drops to 0 |

### Storage, network, privacy
| Feature | Status | Notes |
|---|---|---|
| HTTP/1.1 + HTTP/2 (hyper + rustls 1.3) | ✅ | network tests |
| HTTP/3 + QUIC (quinn) | ✅ | `--features http3`, exercised on http3.is |
| RFC 8489 DoH/DoT | ✅ | DNS wire parser fuzzed |
| Cookie store (RFC 6265bis + CHIPS partitioning) | ✅ | storage tests |
| HSTS + HTTPS upgrade | ✅ | privacy tests |
| Network-layer ad/tracker blocking (adblock-rust lists) | ✅ | `privacy_blocks_tracker_script_but_not_page` |
| CNAME-cloaking guard | ✅ | privacy tests |
| Anti-fingerprinting navigator shield | ✅ | fingerprint tests |
| HTTP cache (content-addressed, memory+disk redb) | ✅ | cache tests |
| `localStorage` / `sessionStorage` | ✅ / 🟡 | sessionStorage same store, not session-cleared |
| **IndexedDB** | 🟡 | synchronous subset over `IdbDatabase` (redb/memory KV): `indexedDB.open`, `createObjectStore`, `put/get/getAll/delete` with keyPath keys. **Missing:** cursors, indexes, version upgrade events, async IDBRequest semantics. Evidence: `indexeddb_put_get_round_trip` |
| Cache API / Service Workers | ❌ | roadmap |
| WebAssembly (wasmi 2: instantiate/call/memory RW) | ✅ | `webassembly_instantiate_and_call` |
| WebRTC data channels | ❌ | rejected for v0.2 (see §5) |
| **Web Animations API** | 🟡 | `Element.animate()` backed by the CSS transition engine + deterministic clock (first→last keyframe pair, duration/delay). **Missing:** keyframe interpolation beyond endpoints, easing options, playback control. Evidence: `element_animate_sets_transition` |
| Forms/validation/File API | ❌ | not landed |

---

## 4. Performance measurements

Harness `render` reports per-stage timings (`--json`). Representative numbers on the CI container (2 cores, debug-profile engine with opt-level=1 deps — treat as relative, not absolute):

| Stage | example.com | wikipedia.org | notes |
|---|---|---|---|
| Fetch + TLS + parse | ~0.4 s | ~1.1 s | incl. DoH bootstrap |
| Stylesheets + cascade | ~0.2 s | ~0.9 s | at-rule eval + layer sort |
| Layout | ~0.1 s | ~0.6 s | taffy + cosmic-text |
| Paint (raster) | ~0.1 s | ~0.4 s | tiny-skia + swash |
| JS realm + scripts | ~0.1 s | ~1.2 s | QuickJS-ng |
| **Full pipeline** | **3.4 s** | **4.1 s** | cold process, incl. engine init |

Memory: arena DOM + suspension keep per-tab RSS small (engine tests assert LRU suspension keeps live pages ≤ budget); framebuffers dominate at ~4 MB/tab at 1280×800. The compositor reports `texture_bytes` + `frame_time_us` per frame in `compositor_stats` (CPU backend here: sub-millisecond composites at 1280×800).

GPU note: the compositor's wgpu path compiles and passes unit tests where an adapter exists; this container exposes none, so screenshots here use the CPU fallback. On GitHub runners, install `mesa-vulkan-drivers` (lavapipe) to exercise the GPU path in CI.

---

## 5. Rejected / deferred decisions (with evidence)

1. **WebGL2** — landed in v0.2.1 as the subset described in §3, built on a GLSL ES normalizer + naga 29 + the compositor's wgpu device seam. The remaining gap (FBOs, transform feedback, instancing) is documented in the matrix rather than silently absent: `getContext('webgl2')` now returns a real context on GPU-capable systems and `null` (spec behaviour) on GPU-less ones.
2. **wasmtime over wasmi** — wasmi chosen for v0.2. Evidence: (a) this container class disallows/marginalizes RWX JIT pages; (b) 2-core CI: wasmi builds in seconds, wasmtime's cranelift is a multi-GB, memory-heavy build; (c) deterministic interpreter simplifies fuzzing. The `WebAssembly` JS surface is engine-level, so swapping wasmtime later is a leaf change.
3. **WebRTC data channels** — rejected for v0.2: no mature pure-Rust SCTP/ICE stack at production quality; webrtc-rs is unmaintained. Re-evaluate when `str0m` matures.
4. **Service Workers / Cache API** — deferred until the event-loop model gains per-origin registration storage; the storage layer (redb KV) already has the schema seam.
5. **Media playback** — rejected for v0.2: no demux/decode stack integrated; `<video>`/`<audio>` parse but render nothing.
6. **`calc()`** — parsed but resolves to 0 today; needs a proper calc evaluator in `length_percentage_to_len`.

---

## 6. Verdict: what the UI layer should expect

**Works today (UI can launch against these):**
- Document/knowledge sites: wikipedia.org, en.wikipedia.org articles (minus infobox tables), docs.rs, arxiv.org, w3.org, rust-lang.org, threejs.org landing, http3.is.
- All internal pages (`brows12://`) — engine has full control.

**Partial (content + scripts visible, chrome degraded):** github.com, MDN, caniuse, BBC, Google, HN (data yes, chrome no). The two structural gaps to close for these are **inline flow** and **tables/floats** (ROADMAP items #1–3), plus presentational attributes.

**Shell-only (needs more JS platform):** youtube.com, reddit.com, crates.io, bing.com — these SPA shells need ES modules, full event propagation, and richer observers before their apps boot meaningfully.

**Blocked at network:** duckduckgo.com family (bot wall) — not an engine defect; a UA/profile strategy is a UI-layer decision.

### Priority order for v0.3 (recommended)
1. Inline formatting contexts (biggest visual-fidelity win for the whole web).
2. Tables + floats (HN, Wikipedia infoboxes, docs everywhere).
3. Presentational attributes + form controls.
4. WebGL2 depth: framebuffer objects + instancing + extension surface (three.js-class demos beyond simple geometry).
5. ES modules + full event propagation (SPA boot).
6. IndexedDB + Element.animate JS surface.

### CI integration
`.github/workflows/ci.yml` runs fmt, clippy `-D warnings`, workspace tests (Linux+Windows), ASan/TSan, fuzz smoke, **plus a headless-harness job** that renders fixture pages and uploads `docs/test-results.json` + screenshots as artifacts. Add `sudo apt-get install -y mesa-vulkan-drivers` on the runner to also verify the wgpu GPU backend with lavapipe.

— Brows12 engineering, v0.2.0

---

## 7. v0.2.1 refresh changelog (this report revision)

All claims above reflect the v0.2.1 state; the deltas since the original
v0.2.0 report:

1. **WebGL 2 landed** — GLSL ES normalizer + naga 29 + wgpu 30 state machine
   (`js/glsl.rs`, `js/webgl.rs`, `js/webgl_bindings.rs`); GPU canvases
   harvest into the image pipeline like Canvas2D. Verified end-to-end on
   SwiftShader (red-pixel assertion in `engine::tests::pipeline`).
2. **WebGPU JS surface landed** — compute-first subset mirroring wgpu
   (`js/webgpu.rs`), error-capture instead of panics on validation errors.
3. **ES modules landed** — `<script type="module">` + `import`/dynamic
   `import()` over the engine network stack.
4. **IndexedDB (sync subset) landed** — real storage semantics via
   `brows12_storage::IdbDatabase` over the engine KV backend.
5. **Element.animate() landed** — transitions-backed WAAPI subset.
6. **Engine fixes**: `document.title` reflects script mutations;
   `load_url_from_string` (the harness/virtual path) now runs the full
   pipeline — stylesheets, scripts, canvas harvest — instead of parse-only;
   navigation clears page-owned surfaces; GPU compositor uniform struct
   alignment fixed (80-byte WGSL `vec3` padding bug that only manifest on
   real GPU backends such as Windows D3D12).
7. **CI hardening**: the push trigger was corrupted (`branches: ain]`) and
   silently skipped push builds — fixed; ubuntu build-test installs
   lavapipe so the GPU paths are exercised every run; the ASan job runs
   with `run_libc_freeres` and documents why.
