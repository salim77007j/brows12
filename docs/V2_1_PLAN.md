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
| G8 | Upstream gaps (documented, unfixed): texture-cache purge on pipeline exit (425 MB residual), `js_mem_max` snapshot-only, image-cache embedder controls, `create_memory_report` crash | `PHASE4_AREA3_REPORT.md` §3.2–3.5, `docs/upstream/` | local forks via the proven `patched/` mechanism + upstream PRs |

## 2. Phase 1 — fix remaining gaps

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

## 3. Phase 2 — innovation: RAM < 100 MB/tab

Protocol for every innovation: measure peak RSS on the heavy-page battery
+ startup + scroll FPS **before** and **after**; ship only if the RAM win
is ≥ 5 % with < 5 % perf cost; record the decision here.

### 2.1 Tiered image decode (expected biggest win)
The `patched/` mechanism is proven (6 crates already forked). Add a
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
Small upstream-style patch: re-apply `JS_SetGCParameter(js_mem_max)` on
`PreferencesUpdated` in servo-script (Area 3 §3.4 verified the snapshot
gap). Effect: backgrounded/hibernate-restored tabs actually shrink their
heaps under the existing tier ladder instead of only new tabs.

### 2.3 Background-tab display-list + cache discipline
- On hide: drop the WebRender display list + frame resources for
  background tabs (rebuild on show; embedder-side, no upstream change);
- periodic `malloc_trim` on the governor tick under Elevated+ PSI
  (extends the existing hibernate-only trim).

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
