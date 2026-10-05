# v2.1.0 Plan — Close the Gaps, Innovate Below 100 MB/tab, Ship Permanents

Status: **plan of record** for the v2.1.0 cycle. Every gap below carries
measured evidence committed in this repo. Baseline = tag `v2.0.0`
(`799ddfb`), release profile (opt3, CGU16, LTO off locally — fat LTO for
the shipped binaries will be produced in CI where the 9.9 GB runners can
link it).

Baseline re-verified this cycle: **108/108 tests green**; site suite
31/32 captures + composites committed (`screenshots/v2-servo/phase5/`);
spot re-runs of example.com / wikipedia / github pass on the fresh
release build.

---

## 1. Remaining gaps (measured evidence)

| # | gap | evidence (artifact) | target |
|---|---|---|---|
| G1 | **cnn.com regression** — brows12 *heavier* than Chrome | peak 481 MB vs Chrome 353 MB = 0.74× (`docs/perf-artifacts/phase5/area5_ram_release.json`) | ≥ 1.0×, ideally ≥ 1.4× |
| G2 | **MDN load-complete not reached** in the 45 s window | `complete=false`, 5,485 frames rendered, title correct (`validation/run/phase5/rw_mdn.json`) — rendering is fine; the completion *criterion* is the problem | load-complete < 5 s or an honest completion criterion |
| G3 | **RAM per tab 114.5 MB** (10× example.com) | `docs/perf-artifacts/phase5/idle_release.json` | **< 100 MB/tab** typical, < 200 MB heavy |
| G4 | 2×-lighter target met on only 1/8 pages | Area 3 §3.7 + phase 5 reruns (1.52–2.07× elsewhere; CNN 0.74×) | ≥ 2× on most pages, never < 1× |
| G5 | bbc.com/news memory sample flaky under the displayless perf harness (load itself now completes) | Area 3 finding + phase 5 reruns | stable memory datapoint |
| G6 | Local release built with LTO off (thin-LTO link cannot finish in any single window on a 3.9 GB box) | v2.0.0 worklog | fat-LTO release artifacts **in CI** |
| G7 | No permanent executables (Actions artifacts expire in 90 days) | — | GitHub Release with 4 platform binaries |
| G8 | Upstream gaps (documented, unfixed): texture-cache purge on pipeline exit (425 MB residual), `js_mem_max` snapshot-only, image-cache embedder controls; note — `create_memory_report` WORKS on the release binary (v2.0.0-era crash not reproducible) | `PHASE4_AREA3_REPORT.md` §3.2–3.5, `docs/upstream/` | local forks via the proven `patched/` mechanism + upstream PRs |

## 2. Phase 1 — fix remaining gaps

> **Status: COMPLETE (this cycle).** Outcomes in §2a below.

### 1.1 CNN regression diagnosis (G1)
Staged memory measurement to attribute the 481 MB: (a) cold engine vs
cnn.com with **images disabled** (privacy allowlist deny-all images),
(b) **JS disabled** env, (c) full load. Compare deltas → attribute to
image decode cache / JS heap / DOM+layout. Prior hypothesis (Area 3):
image decode cache + JS heap with no tiering. Fix at the highest-leverage
layer: if images dominate → Phase 2.1 patch lands early here; if JS heap
dominates → tighten the tier ladder for heavy pages. Verify with the
standard side-by-side battery (Chrome same machine).

### 1.2 MDN load time (G2)
Instrument first: per-resource completion timeline at the embedder
(`load_web_resource` hook already records requests). Expect a tail of
third-party beacons/analytics that keep Servo's "all loads complete"
criterion open forever. Fix honestly at the embedder: **completion
criterion = primary resources + N-second quiet period** (Chrome's
`load` vs `networkidle` distinction), keeping the raw all-complete flag
reported separately in the JSON so the metric is not fudged. If layout
itself is the bottleneck (5,485 frames suggests steady reflow work),
document that architecturally with profile data.

### 1.3 Other documented gaps
- G5: run the bbc memory sample **with Xvfb present** (root cause found:
  displayless mode never reaches completion on that page); make the
  harness always use the display; record the datapoint.
- G8 partial: land the small, clean upstreamable patches locally (see
  2.2/2.4) and file PR drafts under `docs/upstream/`.
- Not fixable this cycle (documented as such): scroll-true-offset
  restoration (needs upstream getter), tab-strip group collapse UI.

## 2a. Phase 1 outcomes (measured, committed)

### G1 CNN — attributed; structural remainder; two fixes landed

Staged attribution (`scripts/p11_cnn_diag.py`, BROWS12_DIAG_DENY,
`docs/perf-artifacts/v21/p11_cnn_diag.json`): peak 490.5 MB full;
**images −14.4 MB (2.9%), scripts −9.7 MB (2.0%), stylesheets −25.3 MB
(5.2%), frames −11.5 MB (2.3%), all combined −14.4 MB** — the page-driven
weight is NOT images/JS. Engine memory report
(`--engine-report`; the create_memory_report hook works on 0.6.0):
explicit 277.9 MB = webrender/images 60.8 + layout box-tree 52.2 +
JS heap ~95 (main 41 + stripe iframe 17.7 + gc 20.6 + non-heap 13.5) +
memory-cache/public 24.8 + DOM nodes 16.4 + image-cache 11.1; the other
~200 MB of peak RSS is non-explicit (display-list/scratch/threads/
allocator). jemalloc retention: freeing 60 MB of textures moved RSS by
only ~14 MB → **peak must be avoided, not reclaimed**.

Chrome floor calibration (`p11_chrome_floor.py`): Chrome tree-PSS is flat
(161–175 MB) across example/HN/cnn — its process-set baseline dominates;
brows12 (single process) is at par on simple pages (180 MB peak). The
regression is exclusively CNN's page-driven part.

Landed:
- `servo-host/src/diag.rs` — BROWS12_DIAG_DENY staged-load deny
  (measurement tool, counters clean), 4 unit tests.
- **fork #7 `patched/servo-net`** — display-bound tiered image decode:
  decoded rasters are rescaled to the physical display bound
  (BROWS12_IMAGE_DECODE_MAX_W/H, default 2560×1600 = 1280×800@DPR2, 0
  disables) before entering the caches; fork-internal tests + 4
  integration tests (`servo-host/tests/decode_cap.rs`). Extreme-cap e2e
  (640×480): webrender/images 62.7 → 41.0 MB, box-tree 54.5 → 37.6 MB —
  pipeline proven. At 2560×1600 the effect on cnn is small (responsive
  srcset already serves ≤1600w) but the cap protects giant-image pages.
- **`network_http_cache_size` 5000 → 256** (patched/servo-config): cnn
  pinned 24.8 MB of response bodies in RAM; 256 unit-weight entries
  bounds the memory cache (memory-cache left the top-8 in the report).
- CNN on product defaults, 3 reps: **473.9 / 477.2 / 476.7 MB** vs
  v2.0.0's 481–490. Honest verdict: **G1 not closed at 1.0×** — the
  remainder is structural (JS heap ~95 MB incl. 5 about:blank adtech
  iframes + stripe; fat layout box-tree; ~200 MB non-explicit). The
  Phase 2 levers (2.2 JS heap tiers, 2.3 background display-list drop,
  2.4 texture purge) attack these; ratio goal re-anchored there.

### G2 MDN — FIXED by embedder completion criterion

Root cause: `LoadStatus::Complete` (document `load` event) is blocked
indefinitely by hung third-party iframes while the page renders fine
(MDN: 2,233–5,485 frames, title correct, raw flag never fires). Fix in
`servo-host/src/delegate.rs` (v2.1 Phase 1.2): embedder criterion =
engine event ∨ **quiet period** (no frames/requests for
BROWS12_COMPLETE_QUIET_MS=2000) ∨ **hard cap** (first-activity +
BROWS12_COMPLETE_MAX_WAIT_MS=15000); the raw flag is still reported
separately (`all_resources_complete`, `complete_criterion`) so the
metric cannot be fudged. 8 unit tests (`tests/completion.rs`; caught a
raw-flag bug before it shipped). Results: MDN **complete in 18.6 s**
(was: never in 45 s), example.com unchanged (473 ms engine event), bbc
unchanged (807 ms engine event), wikipedia unchanged (3.98 s engine
event).

### G5 bbc memory sample — stable via headless path

`scripts/p13_headless_mem.py` (tree sampler around brows-servo, whose
JSON precedes teardown): bbc.com/news = **376.2 MB peak,
engine-load-event at 807 ms** — the previously flaky datapoint is now
reproducible.

### New upstream gaps discovered (G9, G10)

- **G9 engine teardown hang**: MDN (and any page with a hung iframe)
  hangs the engine at shutdown; brows-servo writes its JSON before
  teardown so the harness survives; the perf path stalls before its
  JSON. Pre-existing (v2.0.0 phase5 captures used the same timeouts);
  worked around in the harness, documented for an upstream report.
- **G10 memory-report overflow**: `layout-thread/font-context` rows
  report 17.6 TB (negative size cast) on pages whose iframes load
  webfonts — upstream malloc-size-of/report bug; rows >1 TB are
  filtered in the v21 artifacts.

### Tests

Workspace: **124 passed + 1 ignored** (was 108): privacy 46 + storage 19
+ servo-host 47 (incl. 4 new diag tests) + completion 8 + decode_cap 4.

## 3. Phase 2 — innovation: RAM < 100 MB/tab

Protocol for every innovation: measure peak RSS on the heavy-page battery
+ startup + scroll FPS **before** and **after**; ship only if the RAM win
is ≥ 5 % with < 5 % perf cost; record the decision here.

### 2.1 Tiered image decode (expected biggest win)
> **Status: display-bound cap LANDED in Phase 1** (fork #7,
> `patched/servo-net`); the LRU byte budget + cross-tab dedup below
> remain open for Phase 2, re-scoped against the measured cnn numbers
> (responsive srcset already bounds most real images; the budget then
> mainly targets page-weight outlier pages).
The `patched/` mechanism is proven (7 crates forked). Add a
**decode-to-display-size + decoded-cache LRU + cross-tab dedup** layer:
- decode images at the size they are painted (the display list already
  knows the destination rect);
- cap the decoded cache by bytes (env `BROWS12_IMAGE_BUDGET_MB`,
  default tuned to the per-tab budget); evict LRU decoded entries,
  keep the encoded bytes;
- dedup identical decoded images by (URL, decode-params) in a
  process-global table.
Candidates to patch: the image cache + `pixels` crates consumed by
servo-script; feasibility confirmed during 1.1 profiling.

### 2.2 JS heap tiers — runtime re-apply (upstream candidate)
> **Status: LANDED (fork #8, `patched/servo-script`).** The GC-parameter
> block from `Runtime::new` is extracted into
> `reapply_js_gc_parameters(cx)`; `ScriptThread::handle_msg_from_constellation`
> calls it on every `PreferencesUpdated` (each script thread covers its
> own live runtimes). When the applied ceiling *decreases* process-wide
> it also runs `JS_GC` so the shrink takes effect immediately — verified
> live: the governor's tier change (`Normal → Tight/Small` under
> multi-tab RSS) now re-applies to existing runtimes and collects.
> Bonus fix found while landing: the embedder's `js_mem_max` default was
> 256 * 1024 * 1024 (bytes) but the engine reads the pref as MB clamped
> to [1,256] — the documented 256 MiB ceiling was silently an unbounded
> heap (`JSGC_MAX_BYTES = u32::MAX`). Default is now
> `JS_MEM_MAX_DEFAULT_MB = 256` with a range contract test.

### 2.3 Background-tab display-list + cache discipline
> **Status: RE-SCOPED after attribution — the per-tab offscreen
> rendering context was the real per-tab hog, and the shared context
> (2.3a) closed the <100 MB/tab gate without any display-list surgery.**
> The display-list drop-on-hide remains an upstream candidate for the
> heavy-page peak; the periodic `malloc_trim` already exists on the
> hibernate/reclaim paths.
>
> **2.3a (LANDED): shared offscreen rendering context.** Attribution
> chain (`scripts/p22_ui_memreport.py`, `docs/perf-artifacts/v21/`):
> product 10× example.com = 1146 MB RSS (114.6 MB/tab, reproduced from
> v2.0.0's 1145.4); engine explicit accounting only 34.7 MB for 2 tabs;
> glibc `malloc_stats` proved the rest LIVE (in-use 1395 MB, free-but-
> resident 28 MB) — retention/decay levers were moot; the marginal is
> perfectly linear at ~129 MB/tab and UI-only (headless marginal 11.4).
> The structural difference: the UI gave every tab its own
> `OffscreenRenderingContext` while the (31/32-site-proven) headless
> harness shares one. A/B (`BROWS12_SHARED_CTX` gate, 4 tabs): 580 →
> 349 MB RSS, in-use heap 619 → 274 MB, marginal 129 → 14 MB/tab.
> **10-tab product, shared by default: 446 MB total = 44.6 MB/tab
> (was 114.5; Chrome 126.4 → now 2.83× lighter per tab). Idle CPU
> 0.00%; full interaction smoke PASS (load / switch ×5 / hibernate →
> restore → reload → form-state restore / memreport).**
> Diagnostics added en route: `<MEMREPORT>` FIFO command (engine report
> + RSS + live/suspended counts), `BROWS12_MALLOC_STATS=1` glibc split,
> smaps rollup in `p22`; scripts `p21_idle_ab`, `p22_ui_memreport`,
> `p23_shared_smoke`.

### 2.4 Texture-cache purge (G8, the 425 MB residual)
Local wrapper/patch in the WebRender fork: purge texture-cache entries
belonging to a dropped pipeline (hibernation path). If clean, upstream
PR draft with the Area 3 §3.5 reproduction data.

### 2.5 Considered and rejected (documented decisions)
- **Memory-mapped DOM**: unsound against SpiderMonkey GC rooting and
  stylo's borrowed references; enormous risk for uncertain win.
- **Compressed in-memory display list**: the 2.3 drop-on-hide achieves
  the same memory effect with strictly less complexity; compression only
  pays if we need to *keep* background display lists, which we don't.
- **On-disk paging of large pages**: disk-backed page cache adds
  latency cliffs and privacy questions (cleartext page data on disk);
  hibernation already provides the coarse-grained version safely.

### 2.6 Memory regression gate
New test: load the standard heavy page, assert peak RSS/tab below the
gate (100 MB typical-page / 200 MB heavy-page budget in CI with fixed
page content) — wired as `#[test] #[ignore]` locally (env-dependent) and
a real gate in the Linux CI job.

## 4. Phase 3 — verification (everything, again)

1. 32-site suite re-run against the final build; target 32/32 (G2 fix
   should flip MDN; lobste.rs is a network-level unreachability and will
   be retried); composites to `screenshots/v2.1-final/`.
2. Memory gates: < 100 MB/tab typical, < 200 MB heavy, Chrome battery
   re-run (expect ≥ 1× everywhere if G1 is fixed).
3. Startup < 150 ms, idle 0.00 %.
4. Feature smoke: WebGL/WebGPU/WebRTC/Media/WASM fixtures, privacy
   dashboard counters, tab-innovation e2e (phase4_area4.py --only all).
5. Tests: all green + new tests for every fix/innovation + the 2.6 gate.
6. CI: Linux/Windows/macOS jobs green; sanitizer jobs; the release
   workflow triggered by tags.

## 5. Phase 4 — permanent executables (GitHub Releases)

- CI builds fat-LTO (`lto="fat"`, `codegen-units=1`) for:
  `x86_64-unknown-linux-gnu`, `x86_64-pc-windows-msvc`,
  `aarch64-apple-darwin`, `x86_64-apple-darwin`; binaries stripped,
  each < 50 MB (v2.0.0 single-binary size ≈ 165–172 MB unstripped debug
  included — release+strip expected well under the cap; verified in CI).
- Tag push triggers the release workflow; assets attached to the
  v2.1.0 GitHub Release (permanent), notes from V2_1_FINAL_REPORT.md.
- Post-publish: download each asset, smoke-run the ones runnable in this
  container (Linux; the others are CI-smoke-run), record checksums +
  screenshot of the release page.

## 6. Phase 5 — final report + tag

`docs/V2_1_FINAL_REPORT.md`: before/after for every gap, innovation
ledger with measurements, final site/memory/startup/test numbers, honest
remaining gaps, verdict vs Chrome. Tag `v2.1.0`.
