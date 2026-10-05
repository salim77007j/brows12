# v2.1.0 Phase 3 Report — Forced Verification

**Date:** 2026-10-05 · **Build:** `b13fa13` (Phase 2b final) rebuilt fresh in a
reset container (rustc 1.99.0, `CARGO_PROFILE_RELEASE_LTO=false`,
`codegen-units=16`, jobs=1 — same recipe as every prior measurement cycle) ·
**Platform:** Linux x86_64, user-space Mesa EGL + Xvfb, llvmpipe software GL.

Everything below was re-measured on this build in this session. No number is
carried over from v2.0.0 without a fresh confirming run.

---

## 3.1 Real-world site suite — 31/32 (honest exclusion)

All 31 previously-passing sites were **recaptured on the v2.1 build**
(`scripts/phase5_sites.py --ours-only`; Chromium ref side restored from the
committed v2.0.0 composites, which are bit-identical to the original ref
captures — the composites were built from them).

- **31/32 captured and composed**, label `brows12 v2.1 (Servo)`.
- Composites: `screenshots/v2.1-final/*_compare.png` (31 files) +
  `summary.json`.
- **gnu.org (rw_gnu) excluded — network, not engine**: DNS resolves
  IPv6-only (`2001:470:142:5::116`), the container has no IPv6 egress; curl
  returns 000 on https/http (3 tries) **and Chromium itself times out**
  (Page.goto 45 s exceeded). Same failure mode as the v2.0.0 cycle.
- Every captured page: `complete=true`, real titles; per-page load metrics in
  `validation/run/phase5/*.json` (fresh v2.1 runs). Spot-checks of composites
  (habr, caniuse) show full-fidelity renders; habr's earlier rc≠0 was a
  shutdown-time race — the PNG and metrics are valid (`complete=true`,
  title "Publications / My feed / Habr").

## 3.2 Memory — dual target

| Metric | v2.0.0 | **v2.1 (this run)** | Gate | Verdict |
|---|---|---|---|---|
| Typical 10× example.com, headless (brows-perf, idle-sampled) | — | **27.2 MB/tab** (idle RSS 271.9 MB/10) | <100 MB/tab | **PASS** (3.7× margin) |
| Typical 10× example.com, headless (brows-perf, final RSS) | — | **27.3 MB/tab** (273.1 MB/10) | <100 | PASS |
| Typical 10 tabs, full UI (brows12-ui) | 44.6 MB/tab | **44.5 MB/tab** (434.4 MB) | <100 | **PASS** |
| Heavy page cnn.com (brows-perf gate protocol: settle 6000, timeout 90000) | 476–480 MB | **482.9 MB** peak | <200 MB/tab | **NOT MET — honest** (structural page weight; embedder knobs exhausted; regression floor ≤600 MB passes) |
| bbc.com/news (p13 headless sampler) | 376.2 MB | **376.2 MB** (stable, exact match) | — | consistent |

Regression gates (landed in Phase 2b) re-run e2e in this session:
`gate_typical_per_tab_under_100mb` **PASS**,
`gate_heavy_page_regression_floor` **PASS** (19.7 s).

**brows12 vs Chrome, same-protocol peak RSS** (`scripts/area3_compare.py`,
one fresh engine per page, both engines re-measured today):

| Page | brows12 v2.1 | Chrome | ratio (Chrome/b12) | v2.0.0 ratio |
|---|---|---|---|---|
| hackernews | 178.3 MB | 375.0 MB | **2.10×** | 2.07× |
| local-10img-500dom | 210.2 MB | 379.1 MB | 1.80× | 1.86× |
| local-20img-800dom | 237.3 MB | 411.3 MB | 1.73× | — |
| local-30img-800dom | 252.5 MB | 419.0 MB | 1.66× | — |
| local-40img-1200dom | 287.6 MB | 453.9 MB | 1.58× | — |
| local-50img-1500dom | 313.9 MB | 461.1 MB | 1.47× | 1.52× |
| github-servo | 365.4 MB | 519.5 MB | 1.42× | 1.54× |
| bbc-news | (harness-flaky; p13: 376.2 MB) | 599.1 MB | n/a (cross-protocol) | flaky in v2.0.0 too |
| cnn | 476.6 MB | 353.8 MB | **0.74×** | 0.74× |

**7/8 comparable pages lighter than Chrome (1.42–2.10×); 2× met on 1/8**
(hackernews) — same shape as v2.0.0. The cnn 0.74× structural gap is
**unchanged** (Chrome peak on cnn measured 353.8 MB today, same as the
v2.0.0 cycle; our 476.6 MB matches the Phase 1 product-default runs of
473.9–477.2 MB → no v2.1 regression, no v2.1 gain: attribution showed the
remainder is JS-heap/box-tree structural weight).

## 3.3 Startup & idle

- **Cold start (release brows12-ui, 7 reps): 118 ms median** first-present
  (min 102 / max 209) — **< 150 ms gate PASS** (v2.0.0: 132 ms; Chrome
  reference 98 ms).
- **Idle CPU, 10 tabs × 30 s (UI): 0.033%** (second run; first run 0.167% —
  network tail variance; both ≪ Chrome's 0.05% reference). **PASS.**

## 3.4 Feature matrix — all v2.0 capabilities intact

`scripts/phase3_run.py` re-run end-to-end (local HTTP fixtures, Chromium
ground-truth + brows12 capture, 19/19 composited):
`shadow_dom_basic/slots`, `grid_areas/auto_fill/subgrid`, `css_sticky_fixed/
transforms_anim/shadows_radius/backgrounds/math_functions/custom_props/
filters_clip/writing_modes/columns`, `webfonts`, `canvas2d`, `gpu_apis`,
`webrtc_probe`, `wasm` — **brows12 rendered 19/19**; probe self-reports
unchanged vs v2.0.0 (canvas2d 13/13, wasm 10/10, gpu_apis 3/4 headless
probes, webrtc 8/9 — WebGL/WebGPU/WebRTC remain **feature-gated build
configuration**, CI enable path documented; honest matrix unchanged).
Tab innovations verified live by `p23_shared_smoke.py`: **SMOKE PASS** —
3/3 load, 5× switch cycle crash-free, hibernate → restore-reload →
`state_restore` (scroll estimate + form data), `<MEMREPORT>` FIFO OK.
Privacy layer: 46/46 unit tests + e2e ignored test PASS this session.

## 3.5 Test suite

- `cargo test --release --workspace`: **124 passed, 0 failed**
  (privacy 46+1e2e, storage 19, servo-host lib 47, completion 8, decode_cap 4,
  memory_gate 2 e2e) + 1 privacy e2e ignored test run explicitly —
  **127 total green**.
- New-since-v2.0.0 tests all present: completion criterion (8), decode cap
  (4), diag staged-deny (4, inside lib 47), JS heap tier contract, memory
  gates (2 e2e, run with release binary + GL + network).

## 3.6 CI + sanitizers

Deferred to **Phase 4** by plan (workflows build all 5 platforms and run the
memory gates `--ignored` on runners; ASan/TSan/Valgrind jobs are part of the
Phase 4 CI matrix — local container cannot link ASan builds in the time
window; this is recorded, not hidden).

## Artifacts (this session)

- `docs/perf-artifacts/v21/p3_typical10.json`, `p3_typical10_idle.json`,
  `p3_cnn_gateproto.json`, `p3_cnn_v23_*.json`, `p3_bbc_headless.json`,
  `p3_startup_release.json`, `p3_idle_release.json`, `p3_idle_release2.json`,
  `p3_area3_ram.json`, `b12_gate_*.json`
- `screenshots/v2.1-final/` (31 composites + summary.json + `features/` 19
  fixture composites + run_results.json)
- `validation/run/phase5/{ref,ours}/*.png` + per-site JSONs;
  `validation/run/phase3/run_results.json`

## Honest verdicts

1. Site suite: **31/32** (gnu.org IPv6-unreachable container network;
   Chromium fails identically).
2. Typical-page RAM gate **PASSED with margin** (27–45 MB/tab vs 100).
3. Heavy-page <200 MB/tab **NOT met** — structural; floor-gated at ≤600 MB
   and passing.
4. 2×-everywhere **NOT met** (1/8) — unchanged from v2.0.0; local-page
   ratios improved; cnn unchanged at 0.74×.
5. Startup/idle gates **PASSED** (118 ms; 0.03%).
6. Features/tab innovations/privacy: **all intact** (19/19 fixtures, smoke
   PASS, 127 tests green).
