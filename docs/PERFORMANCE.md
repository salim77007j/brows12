<!-- v1 custom-engine content is preserved below. The v2 (Servo) section
     on top reflects the Phase 2 memory/performance work and its
     Chrome-comparison methodology. -->

# Performance — v2 (Servo engine), Phase 2 report

All v2 numbers were measured on the CI container (2 vCPU, ~3 GB RAM,
software GL via Xvfb/swrast, no GPU) with `/proc` ground truth —
`VmRSS` from `/proc/self/status` and `(utime+stime)` from
`/proc/self/stat` — not engine-internal counters. Scripts:
`scripts/measure_idle.py`, `scripts/measure_governor.py`,
`scripts/measure_startup.py`, `scripts/measure_chrome.py` (in the
repo); raw JSON artifacts referenced inline.

Reference page for the comparison table: `https://example.com/`
(lightest realistic page; 10 tabs = 10 independent loads).

## Mission targets vs results (Phase 2 gates)

| Metric | Target | brows12 v2 result | Verdict |
|---|---|---|---|
| Idle CPU, 10 tabs | < 1% | **0.00%** (30 s window; was 0.267% before Phase 2) | **Met** |
| Suspended tab RAM | < 20 MB | **~11 MB marginal/tab** (9 background tabs hibernated: 1.07 GB → 390 MB with `malloc_trim`) | **Met** |
| Cold start | < 500 ms | **195 ms** median to first presented frame (5 runs: 181–211 ms) | **Met** |
| Per-tab RAM (live) | < 50 MB | **~90 MB marginal/tab** live (1.07 GB for 10 live tabs) | **Not met — see gap analysis** |
| Scroll 60 FPS | 60 FPS | **34.7 FPS** under software GL (60 Hz wheel input, Wikipedia article) | **Partial — software-GL caveat** |

## brows12 v2 vs Chromium (Playwright headless_shell), same box, same page

| Metric | brows12 v2 (Servo) | Chromium headless |
|---|---|---|
| 1-tab RSS | **289 MB** | 366 MB |
| 10-tab RSS (all live) | **1.07 GB** | 1.22 GB |
| Per-tab marginal (live) | ~90 MB | ~95 MB |
| Idle CPU, 10 tabs | **0.00%** | 0.05% |
| 10 tabs, 9 suspended by governor | **390 MB** | not applicable (headless build has no discard path) |
| Cold start | 195 ms to first presented frame* | 114 ms launch→`domcontentloaded`* |
| Scroll FPS, Wikipedia article | 34.7 FPS (software GL) | not comparable (GPU) |

\* The startup numbers use different yardsticks and are NOT directly
comparable: Chrome's figure ends at DOMContentLoaded for a cached trivial
page; ours ends at the first *presented composited frame* including
winit window + GL context + WebRender first frame + chrome draw. We
report both honestly rather than pretend one number.

## What produced the wins (and the honest remaining gaps)

1. **Event-driven idle (2.3).** The Phase 1 shell pumped the event loop
   at 16 ms unconditionally. The v2 loop pumps at 16 ms only while a tab
   is loading or the active page reports animating
   (`WebViewDelegate::notify_animating_changed`); otherwise the loop
   sleeps in `ControlFlow::Wait` until the engine wakes it through the
   embedder waker. Result: 10 idle tabs cost **0.00% CPU** (measured over
   30 s). Background tabs additionally run under `WebView::set_throttled(true)`
   — Servo clamps their timers and stops their animations engine-side
   (`js_timers_minimum_duration` kept at the 1000 ms clamp).

2. **Tab hibernation + memory governor (2.1).** Hibernation =
   `set_throttled(true)` + `hide()` + drop the `WebView` handle — its
   `Drop` sends `CloseWebView` and the constellation tears the document
   pipeline down. The shell keeps title/URL and restores by reloading.
   The governor samples `VmRSS` on its own `WaitUntil` cadence inside
   `about_to_wait` (the only winit callback fired when a timer expires on
   an idle system — a subtle trap we hit and documented: a governor
   living in the general tick path starves when idle). Over budget it
   hibernates least-recently-active background tabs until back under
   budget or out of candidates. Env knobs (all documented in
   `servo-host/src/memory.rs`): `BROWS12_MEM_BUDGET_MB` (default 384),
   `BROWS12_GOVERNOR=0`, `BROWS12_GOVERNOR_INTERVAL_MS`,
   `BROWS12_TAB_SUSPEND_SECS` (default 180).
   Measured reclaim: 10 × example.com, peak 961 MB → **390 MB** after
   hibernating 9 tabs (governor events in the JSON artifact), i.e. the
   whole 10-tab session collapses to ~1.35× the single-tab footprint.

3. **Allocator return (`malloc_trim(0)`).** Without it, freed engine
   memory sat in retained arenas and `/proc` showed no reclaim
   (580 MB vs 390 MB — a 190 MB difference). `malloc_trim(0)` runs after
   each governor hibernation batch. This is an embedder-side technique
   that any Servo embedder wanting a memory governor will need; it is
   brows12 policy, not an engine patch.

4. **Engine cache bounds (prefs, not patches).**
   `network_http_cache_size` 5000 → 1024 entries (evictions spill to the
   disk cache — see servo-net `http_cache.rs::on_evict`), `js_mem_max`
   -1 (unlimited) → 256 MiB. Every knob's rationale is in
   `servo-host/src/prefs.rs::brows12_preferences`.

5. **Startup (2.2).** The chrome (tab strip + toolbar + omnibox) presents
   immediately after boot (`request_redraw` right after engine build),
   so first paint does not wait for the first engine frame. Measured:
   195 ms median to first present; Servo's own `build()` is 3 ms (lazy
   subsystem init) and the first engine frame of example.com lands at
   ~130–190 ms headless.

### Gap analysis (honest)

- **Live per-tab RAM (~90 MB marginal) misses the <50 MB target.** The
  headless harness measures only ~3.5–6 MB marginal per additional tab of
  the same page, so the bulk of the shell's per-tab cost is per-WebView
  display/rendering structures (per-tab offscreen contexts + WebRender
  document state), not page content. The governor is the mitigation that
  keeps real sessions bounded (390 MB for 10 tabs); shrinking the live
  per-tab cost further needs engine-side work (shared compositing for
  offscreen webviews) and is a Phase 4.5 upstream-investigation item.
- **Suspension is not freeze.** Hibernation frees the pipeline but a
  restore reloads (network cache makes this cheap); scroll position and
  form state are not preserved (v1-era behavior, unchanged).
- **Scroll 34.7 FPS is a software-GL number.** WebRender composites on
  the GPU in production (WindowRenderingContext on a composited desktop);
  the CI container has no GPU (swrast), so the 60 FPS gate can only be
  validated on real hardware — documented, not claimed.
- **The engine memory reporter crashes servo 0.6.0.**
  `Servo::create_memory_report()` panics the SystemFontService (usize
  add overflow in `servo-malloc-size-of` traversal) — an upstream bug
  found during Phase 2; the flag is off by default in `brows-perf`
  (`--engine-report` opts in) and the patch+PR is queued for Phase 4.5.
  All memory numbers above therefore use `/proc` ground truth.

# Performance — v1 (custom engine, historical)

## Methodology

Numbers in this document come from two sources, both committed to the repo:

1. `brows12-compare` (`benchmarks/src/bin`) — engine-side timings that map
   1:1 to user-visible operations: cold engine construction, tab creation,
   first paint of an internal page, and a full pipeline load against a local
   HTTP server. CI runs it on every commit and appends the JSON to artifacts.
2. Criterion micro-benchmarks (`cargo bench -p brows12-benchmarks`) — HTML
   parse throughput, CSS parse, full-document cascade, layout, raster, JS
   realm operations, cookie jar and cache operations.

Cross-browser comparison uses `benchmarks/scripts/compare_browsers.sh`,
which measures Chrome/Firefox/Brave cold start (`hyperfine`, headless
`about:blank`) and appends JSON per browser. Browser numbers are only
comparable within one machine; the script exists so anyone can reproduce —
we deliberately do not publish cross-browser tables we cannot regenerate.

## Engine-side baseline (debug build, development container, 2 vCPU)

| metric | value |
|---|---|
| cold start — engine construction (TLS stack, font scan) | 23 ms |
| tab creation | < 1 ms |
| internal page (`brows12://`) first paint | 11 ms |
| local page full pipeline (fetch → parse → cascade → layout → paint) | 6 ms |

Release builds (`lto = "thin"`, `codegen-units = 1`) shrink these further;
the CI bench job publishes up-to-date artifacts.

## Where the wins come from

### Startup

- No JIT to warm: QuickJS-ng starts in microseconds per realm.
- No GPU initialization before the first frame (CPU raster path).
- Font system scan happens once per engine, shared by all tabs
  (`Arc<Mutex<FontSystem>>`).
- Everything heavy is lazy per tab; the engine itself wires ~5 subsystems.

### Memory

- Arena DOM: one `Vec<Node>` per document; teardown is O(1) and refunding
  to the allocator is immediate (no Rc cycles to collect).
- Display-list rendering: paint state is flat and pre-sorted; no retained
  layout objects beyond the rect map.
- Tab suspension is the big lever: a suspended tab is a URL string, a title
  and an optional snapshot. The LRU sweep (`Engine::enforce_memory_budget`)
  keeps resident pages within `max_live_pages` automatically.
- Response bodies > 1 MiB skip the memory cache (disk layer still caches).
- JS realms: 256 MiB hard heap cap each, enforced by QuickJS's allocator
  integration — a runaway page fails alone.

### Throughput

- lightningcss parses at multiple hundreds of MB/s on commodity hardware;
  cascade matching is the O(rules × elements) hot path and is benchmarked
  explicitly (`cascade/60_sections_full_document`) so regressions are loud.
- Layout calls into taffy's cached, incremental-friendly solver; text
  measurement is the only external call and is bounded by leaf count.
- tiny-skia paints 1280×720 pages in single-digit milliseconds on 2 vCPU
  (bench `render/60_sections_raster_1280x720`).
- Glyphs render through a swash image cache keyed by (font, glyph, size,
  subpixel bin) — shaping runs once per measurement, blitting runs per
  frame.

## Optimization backlog (tracked in ROADMAP)

- Bloom-filtered rule matching (ancestor hashes) for cascade.
- Incremental relayout (dirty-node taffy updates instead of full rebuild).
- Glyph atlas persistence across frames (avoid re-blit of static text).
- vello GPU tiles behind the compositor seam.

