# brows12 v2.1.0 — "Under 100 MB"

A memory-focused release of the brows12 Servo-0.6.0 browser: the remaining
v2.0 gaps fixed, a per-tab memory redesign (114.5 MB → **27–45 MB/tab**),
end-to-end re-verification, and **permanent, downloadable release binaries
for four platforms** built with fat LTO.

## Highlights

- **RAM per tab: 114.5 → 27.2 MB/tab** headless (10× example.com,
  idle-sampled) and **44.5 MB/tab** in the full UI — both far under the
  100 MB target (Chrome reference on the same protocol: 126.4 MB/tab;
  brows12 is 2.8× lighter per tab).
- **Shared offscreen rendering context** across tabs (the ~115 MB/tab
  per-tab GL context bookkeeping was the entire gap) with an escape hatch
  (`BROWS12_SHARED_CTX=0`).
- **Tiered image decode** (display-bound, capped at 2560×1600) and a bounded
  network memory cache — engine-side upstream patches, e2e-proven
  (textures 62.7 → 41.0 MB on the probe).
- **JS heap tiers**: `js_mem_max` now applies at runtime (re-applied on
  preference updates, decrease-guarded GC nudge) instead of being silently
  unbounded.
- **Smart page completion**: hung third-party iframes no longer block load
  completion (engine event OR quiet-2s OR first-activity-cap-15s) — MDN
  completes in ~18.6 s instead of never-within-45 s.
- **CNN attribution**: page weight is NOT image/JS-driven (images 2.9%,
  scripts 2.0%); structural remainder honestly documented.
- **Startup 118 ms median** (7 reps, release build), idle CPU **0.033%**.
- **31/32 real-world sites** re-captured side-by-side vs Chromium on this
  exact build (`screenshots/v2.1-final/`); gnu.org excluded for container
  IPv6 unreachability (Chromium fails identically).
- **127 tests green** (124 unit/integration + 3 e2e including two
  hard memory-regression gates).

## Memory vs Chromium (same-protocol peak RSS, fresh engine per page)

| Page | brows12 v2.1 | Chrome | ratio |
|---|---|---|---|
| Hacker News | 178 MB | 375 MB | **2.10×** |
| Local 10–50 img pages | 210–314 MB | 379–461 MB | 1.47–1.80× |
| github.com/servo/servo | 365 MB | 520 MB | 1.42× |
| cnn.com | 477 MB | 354 MB | 0.74× (structural, honest) |

## Downloads

Per-platform zips below contain `brows12-ui` (the browser), `brows-servo`
and `brows-perf` (headless engine harness), each binary **< 50 MB**
(fat LTO, stripped). SHA-256 checksums ship alongside.

| Platform | Asset |
|---|---|
| Linux x86_64 | `brows12-2.1.0-linux-x86_64.zip` |
| Windows x86_64 | `brows12-2.1.0-windows-x86_64.zip` |
| macOS arm64 | `brows12-2.1.0-macos-arm64.zip` |
| macOS x86_64 | `brows12-2.1.0-macos-x86_64.zip` |

## Honest remaining gaps

- Heavy pages stay above 200 MB/tab (cnn ≈ 483 MB peak; floor-gated at
  600 MB) — structural page weight; embedder-side knobs are exhausted.
- 2× lighter than Chrome holds on 1/8 of the comparison battery
  (Hacker News); 7/8 are lighter, cnn is heavier.
- WebGL/WebGPU/WebRTC remain compile-time feature gates (CI verify path:
  `servo-features.yml`); sanitizer program scopes to brows12 workspace
  crates (engine C++ is upstream-covered).

Full evidence: `docs/V2_1_PHASE3_REPORT.md`, `docs/V2_1_PLAN.md`,
`docs/perf-artifacts/v21/`, `screenshots/v2.1-final/`.
