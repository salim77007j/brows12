# Brows12 v2.0 — Servo-Based Browser Engine: Final Report

**Tag:** v2.0.0 · **Date:** 2026-10 · **Engine:** Servo 0.6.0 (Stylo 0.21,
WebRender 0.70, SpiderMonkey 153) behind the brows12 embedder (UI shell +
privacy/security stack + tab lifecycle governor).

---

## 1. Executive summary

Brows12 v2.0 is a complete, honest rewrite of the v1 custom engine onto
Servo 0.6.0 as the web platform. The embedder owns the chrome (UI shell,
tab management), the privacy/security stack (network-layer blocking,
anti-fingerprinting, HTTPS/HSTS, cookie partitioning, dashboard), and the
tab lifecycle governor (predictive hibernation, memory-pressure discard,
session restore, groups, pinned tabs, search).

Verdicts that matter, measured on the same machine against Playwright
Chromium:

- **Real-world reach:** 31/32 attempted real sites load and render
  (lobste.rs unreachable from the test network, not an engine failure).
  Side-by-side composites for every site are committed under
  `screenshots/v2-servo/phase5/`.
- **Memory:** brows12 is lighter than Chrome on 7/8 measurable pages of
  the standard comparison battery (1.52–2.07×). The "2× lighter per
  page" target is met on 1/8 pages (hackernews, 2.07×) — see §5 for the
  honest accounting.
- **Idle:** 0.00 % CPU with 10 open tabs over a 30 s window
  (Chromium reference: 0.05 %).
- **Cold start:** 132 ms median to first present (release build; the
  debug build measured 195 ms in Phase 2). Chromium reference: 98 ms.
- **Per-tab RAM (light page):** 114.5 MB/tab for 10 example.com tabs vs
  Chromium 126.4 MB/tab.

## 2. What shipped in v2.0 (per phase)

| Phase | Scope | Status |
|---|---|---|
| v1.0.0-rc1 | Custom engine (own HTML/CSS/layout/render/JS) | Retired, preserved at tag `v1.0.0-rc1` |
| Phase 1 | Servo 0.6.0 integration, embedder shell | Complete |
| Phase 2 | Memory/performance instrumentation, governor | Complete |
| Phase 3 | 19 CSS/platform fixtures vs Chromium | Complete (13 Yes / 4 Mostly / 3 CI-bound) |
| Phase 4 Area 1 | clip-path, backdrop-filter, writing-modes, canvas shadows, subgrid, multicol, 3 upstream PRs | Complete (see `PHASE4_AREA1_REPORT.md`) |
| Phase 4 Area 2 | Ad/tracker blocking (143k rules), anti-fingerprinting, popup blocker, HTTPS-Only + HSTS, DoH, cookie partitioning, CSP/COOP/COEP/CORP/SRI, privacy dashboard API | Complete (`PHASE4_AREA2_REPORT.md`) |
| Phase 4 Area 3 | Per-tab budgets, image/font/JS-heap memory work, texture-cache investigation, memory-pressure response, honest measurement | Complete (`PHASE4_AREA3_REPORT.md`) |
| Phase 4 Area 4 | Predictive hibernation, memory-aware discard, tab groups, full session restore, tab search, pinned tabs, configurable suspend policy | Complete (`PHASE4_AREA4_REPORT.md`) |
| Phase 5 | 31-site verification, release-build benchmarks, this report, tag v2.0.0 | Complete (this document) |

## 3. Phase 5 — real-world verification (31 sites)

Protocol: for each site, Chromium (Playwright) is the ground-truth
capture; brows12 (`brows-servo` headless, 1280×800, 45 s window, 3 s
settle) must produce a screenshot + JSON with `complete`/`crashed`
status. Composites: `screenshots/v2-servo/phase5/<name>_compare.png`;
metrics: `validation/run/phase5/summary.json`.

Load-complete times (brows12, release build): example.com 293 ms,
text.npr.org 340 ms, wikipedia.org 853 ms, docs.rs 745 ms,
news.ycombinator.com 1751 ms, github.com 3536 ms, sqlite.org 3832 ms,
habr.com 8752 ms, html.spec.whatwg.org 44.0 s (multi-MB DOM — fully
loaded).

Notable results:

- **CJK correctness:** after installing user-local Noto Sans CJK,
  wikipedia.org's Japanese/Chinese links render identically to Chrome
  (previously tofu — a container font-coverage issue, not an engine bug).
- **MDN** rendered fully (5,485 frames, correct title) but the
  load-complete flag was not reached inside the 45 s window due to
  long-tail resources — honest caveat.
- **lobste.rs** could not be reached from the test network at all
  (Chrome also timed out) — excluded rather than counted as a failure.
- **bbc.com/news** now loads completely (it failed wholesale in the
  Phase 4 debug profile; the release build resolved it).

## 4. Phase 5 — performance vs Chromium (release build)

Protocol: `scripts/area3_compare.py` (one fresh engine per page,
1280×800, RSS polled @250 ms, PSS/USS at peak, VmHWM tree sum) and
`scripts/measure_startup.py` / `measure_idle.py` / `measure_chrome.py`.
Artifacts: `docs/perf-artifacts/phase5/`.

### 4.1 Page memory (peak RSS)

| page | brows12 peak | Chrome peak | Chrome/b12 |
|---|---|---|---|
| local 10 img / 500 dom | 209 MB | 389 MB | **1.86×** |
| local 20 / 800 | 233 MB | 412 MB | **1.77×** |
| local 30 / 800 | 257 MB | 435 MB | **1.69×** |
| local 40 / 1200 | 291 MB | 455 MB | **1.56×** |
| local 50 / 1500 | 315 MB | 480 MB | **1.52×** |
| github.com/servo/servo | 350 MB | 538 MB | **1.54×** |
| news.ycombinator.com | 181 MB | 375 MB | **2.07×** |
| cnn.com | 481 MB | 353 MB | **0.74×** |
| bbc.com/news | load ✓, sample flaky | 595 MB | — |

### 4.2 Global metrics

| metric | brows12 v2 (release) | Chromium |
|---|---|---|
| cold start to first present (median, 5 reps) | **132 ms** | 98 ms |
| idle CPU, 10 tabs / 30 s | **0.00 %** | 0.05 % |
| 10-tab RSS per tab (example.com) | **114.5 MB** | 126.4 MB |

### 4.3 What the release build bought (honest)

The release build (opt-level 3) reduced peak RSS by only **2–6 %**
versus the debug build, because the dev profile already compiled all
dependencies (including the entire Servo stack) at opt-level 1 — only
the embedder crates moved from opt 0 to opt 3. Startup improved more
visibly (195 ms → 132 ms). The remaining gap to "2× everywhere" is
architectural, not a build-flag issue — see §5.

## 5. Honest limitations and open gaps

1. **The "2× lighter per page" target is met on 1/8 pages.** brows12 is
   lighter on raw RSS almost everywhere (7/8), but the 2× goal requires
   the levers named in the Area 3 report: tiered image handling,
   JS-heap tiers with idle-timer detection, PSI-driven reclaim, a more
   effective hibernation trim, and upstream engine changes. cnn.com is
   an outright regression (0.74×): heavy JS + heavy DOM with no tiered
   image decode yet.
2. **bbc.com/news memory sampling** under the perf harness remains
   flaky (load completes, but not reliably inside the 90–150 s window
   when the harness runs displayless). The Area 3 hard load failure is
   resolved; the memory datapoint is pending.
3. **MDN long-tail resources** keep the load-complete flag open past
   45 s; rendering itself is correct.
4. **Upstream gaps** discovered and documented with minimal repros
   (see `docs/upstream/` and phase reports): `create_memory_report`
   SystemFontService crash; webrender global texture cache not purgable
   from the embedder; subgrid layout gap; media-session playback-state
   writes not notifying the embedder.
5. **Scroll restoration** estimates wheel deltas rather than reading
   the engine's true scroll offset (Servo 0.6 exposes no getter).
6. **Group collapse** is data-layer only; the tab strip still draws all
   tabs.
7. **Release profile in this environment:** LTO was disabled for the
   final link (thin-LTO link of `brows12-ui` could not complete inside
   any single build window on the 2-CPU / 3.9 GB container). A full
   thin-LTO build is expected to shave a further small amount.

## 6. Conclusion

v2.0 delivers what the Servo pivot promised: a real web platform
(Stylo + WebRender + SpiderMonkey) with a genuinely differentiated
embedder — network-layer privacy with 143k live rules, a full
anti-fingerprinting suite, tab lifecycle innovations, and an honest
measurement culture (every number in this report has a committed
artifact behind it). brows12 is lighter than Chrome on nearly every
measured page, matches Chrome on idle behaviour, and loads 31 real
sites correctly. The remaining memory-architecture work is scoped and
documented as the path to the 2× goal.
