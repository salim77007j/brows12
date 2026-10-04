# Phase 4 — Focus Area 3 Report: Aggressive RAM Optimization

Status: **complete** (3.1–3.7). Build: debug (Phase 2/3 measurement
convention, honestly labeled). All work committed per sub-item; artifacts
under `docs/perf-artifacts/phase4/area3/`.

---

## 3.1 Per-tab memory budget + graceful degradation — DONE

New `servo-host/src/budget.rs` (pure policy, unit-tested) + wiring in the
shell governor (`ui/src/main.rs::governor_tick`).

- **Per-tab weight estimate**: `tab_base_kb (8 MB) + page_requests ×
  per_request_kb (256 KB)` — the same request-count proxy the governor
  has used since Phase 2; it ranks tabs and drives the ladder, it does
  not pretend to be a measurement (per-tab engine accounting is not
  exposed by Servo 0.6.0 — upstream gap, §3.5/§3.7 below).
- **Availability-adaptive total budget**:
  `total = min(nominal_budget, baseline 96 MB + MemAvailable × 0.25)` —
  the browser shrinks its own ceiling when the *system* is constrained,
  reads `MemAvailable` from `/proc/meminfo` each tick.
- **Active-tab protection**: the active tab gets `active_share × (3.0)`
  the fair share of the remaining pool, floored at its own weight — the
  tab being looked at is never degraded while it is within its budget.
- **Graceful-degradation ladder** (`Degradation::decide`):
  within budget → nothing; over → `TrimCaches` (hide + throttle +
  `malloc_trim`); still over after a 30 s grace → `Hibernate` (drop the
  pipeline; restore = rebuild + reload, existing Phase 2 mechanism).
  A trimmed tab is un-hidden and un-throttled the moment the user
  activates it (`switch_to` fix).

Verification: 5 unit tests (`degradation_ladder`, budget share,
availability shrink) + live event stream (`docs/perf-artifacts/phase4/
area3/area3_runA_events.json`): at Critical the governor hibernated the
inactive start tab, then the over-weight background heavy tabs on the
heavy schedule; `governor_global level=Critical reclaimed=…` emitted.
The live trim *step* was preempted by the global reclaim at Critical
(hibernation is strictly stronger than trimming, so it wins the race);
the trim step itself is unit-verified and reachable at Elevated pressure
on over-budget light tabs.

## 3.2 Image memory — hibernation evicts; upstream owns the rest

- **Implemented/embedder-side**: image memory is bounded by the
  hibernation policy. Measurement (2 tabs × 20 PNGs × 1 MB decoded,
  debug): after `<HIBERNATE>` of both background tabs **122.7 MB
  returned** (`freed_by_hibernate_mb`), after the fix below.
- **Fix shipped this Area**: `hibernate()` now calls `malloc_trim` on
  *every* hibernation (previously only the governor loop did, so
  command-driven hibernation left freed arenas allocator-retained).
  35.5 MB → 122.7 MB returned on the same scenario — 3.5×.
- **Upstream gaps** (image cache lives in the un-patched `servo` crate;
  documented for upstream filing): viewport-only lazy decode, decode
  downsampling to display size, cross-tab dedup of identical decoded
  images. Servo 0.6.0's image cache has no embedder controls for any of
  the three.

## 3.3 Font memory — sharing/retirement exist; subsetting is upstream

Full investigation: `docs/upstream/area3-font-memory.md`. Summary:
cross-tab font-data sharing is **inherent** (process-global FontStore +
WebRender font keys); per-display-list retirement is **wired end-to-end**
(layout collects unreferenced keys → `PaintApi::
remove_unused_font_resources` → WR drops texture-cache entries); what is
missing upstream: glyph subsetting (whole-blob caching pins large CJK
families), an N-second unload policy for idle font faces, and working
font accounting (`create_memory_report` crashes in SystemFontService —
known bug, blocks measurement).

## 3.4 JS heap — dynamic tiers + idle-timer detection — DONE

- **`JsHeapTier` policy** (`budget.rs`): Normal 256 MB (prefs default) →
  Tight 192 → Small 128 → Minimal 96, chosen from RSS ratio, PSI heat,
  and over-budget-tab count; applied engine-wide via the public
  `Servo::set_preference("js_mem_max", …)`.
- **Caveat (verified in servo-script 0.6.0 source)**: `js_mem_max` is
  snapshotted at JS-runtime creation (`JS_SetGCParameter` once); live
  runtimes are unaffected — every runtime created *after* the tier
  change (i.e. every tab restored from hibernation under pressure)
  inherits the smaller heap. Runtime re-application is an upstream
  candidate (small patch to script_thread's PreferencesUpdated handler).
- **Idle-timer detection**: `HostState.last_activity_ms` (unix-ms) is
  touched by `notify_new_frame_ready` (rAF/CSS animation/timer repaints)
  and by every resource request (fetch/XHR polling). A background tab
  active within 5 s is treated on the heavy suspend schedule — burning
  CPU while invisible gives up its pipeline sooner.
- Live verification: `js_heap_tier tier=Small mem_max_mb=128
  rss_ratio=1.81/3.48` events under Critical (run A artifact).

## 3.5 WebRender texture-cache management — residual quantified

Phase 3 found hibernation cannot purge the WebRender-side caches; this
Area **quantified** it. Protocol (`scripts/area3_verify.py` run B/C,
governor off, manual hibernation): baseline = start page only; load two
image-heavy tabs; move to a light active tab; hibernate both heavy tabs.

| point | RSS |
|---|---|
| baseline (start page) | 210.7 MB |
| both heavy tabs loaded | 690–711 MB |
| both hibernated (with trim fix) | 635.9 MB |
| **residual vs baseline** | **425.3 MB** |

Reading (honest): the DOM/JS/image side of the pipelines is returned
(122.7 MB directly observed freed); what remains is dominated by
GL/WebRender-side state and allocator retention the engine does not
release on WebView drop — consistent with the Phase 3 attribution of a
global texture cache that no embedder API can purge. The engine-side
levers (purge-on-pipeline-exit, per-context texture accounting) require
upstream changes; upstream issue to file (see §Upstream).

## 3.6 Memory-pressure response (PSI) — DONE

New `servo-host/src/psi.rs` + governor integration.

- Reads `/proc/pressure/memory` (system-wide) and cgroup v2
  `/sys/fs/cgroup/memory.pressure` (the tighter scope wins when
  present); parses `some`/`full` avg10/avg60.
- Mapping: `full ≥ 5 %` (avg10) or `≥ 2 %` (avg60) → Critical (real
  thrashing); `some ≥ 25 %` (avg10) → Elevated (sustained contention);
  thresholds env-tunable (`BROWS12_PSI_*`), master switch `BROWS12_PSI`.
- The governor acts on **worse-of(own-RSS level, PSI level)**: the
  browser now yields memory when the *system* thrashes even if its own
  budget is clean, and tightens the JS heap on PSI heat — with a
  cooldown (`BROWS12_PSI_COOLDOWN_MS`, default 10 s) so a sustained
  spike reclaims at a humane pace instead of hibernating the whole strip
  in one tick.
- 5 unit tests (both file formats, garbage input, level mapping, env
  overrides). Live PSI stayed Nominal in-container (expected — the box
  was not thrashing); the trigger path is exercised by the shared
  reclaim code path (`reclaim_background_tabs`).

## 3.7 Measurement & honesty — the 2× target is NOT met

`scripts/area3_compare.py` + `docs/perf-artifacts/phase4/area3/
area3_ram.json`. Same machine, same pages, one fresh engine per page,
1280×800; peak = VmHWM/250 ms-polled maximum; PSS sampled at the peak.
brows12 = debug build (Phase 2/3 convention); Chrome = Playwright
Chromium (release).

| page | brows12 peak | Chrome peak | Chrome/b12 | brows12 PSS | Chrome PSS |
|---|---|---|---|---|---|
| local 10 img / 500 dom | 223 MB | 393 MB | **1.76×** | 228 MB | 195 MB |
| local 20 / 800 | 247 MB | 413 MB | **1.67×** | 252 MB | 213 MB |
| local 30 / 800 | 273 MB | 433 MB | **1.58×** | 280 MB | 231 MB |
| local 40 / 1200 | 296 MB | 457 MB | **1.55×** | 300 MB | 248 MB |
| local 50 / 1500 | 331 MB | 472 MB | **1.43×** | 335 MB | 270 MB |
| github.com/servo | 370 MB | 530 MB | **1.43×** | 370 MB | 295 MB |
| news.ycombinator.com | 185 MB | 376 MB | **2.03×** | 190 MB | 175 MB |
| bbc.com/news | **load fails** | 597 MB | — | — | 372 MB |
| cnn.com | 471 MB | 352 MB | **0.75×** | 474 MB | 162 MB |

**Verdict (honest):**
- Raw RSS: brows12 is lighter than Chrome on 7/8 measurable pages
  (1.43–2.03×) — but only **1/8 pages meets the "2× lighter per page"
  target** (hackernews, 2.03×).
- On the fairest metric (PSS, which does not double-count Chrome's
  shared pages): parity on the light end, ~15–25 % heavier on the heavy
  local pages, clearly heavier on github-servo.
- cnn.com regresses badly (0.75×): heavy JS + heavy DOM in a **debug**
  SpiderMonkey/Stylo with no tiered image handling.
- bbc.com/news: brows12 load failure (new finding; separate
  investigation — likely JS/feature gap on a heavy real page, not a
  memory issue).

**Named causes and the levers that would close the gap** (ordered by
expected impact): (1) release/LTO build of brows12 — debug engines carry
multi-MB debug arenas everywhere and it is the single largest dishonest
cost in these numbers; (2) image decode downsampling + lazy decode
(upstream); (3) texture-cache purge on pipeline exit (upstream, §3.5);
(4) runtime JS-GC re-application of `js_mem_max` (upstream, §3.4);
(5) per-tab engine accounting so budgets can act on real numbers
(`create_memory_report` crash fix, upstream — also an Area 1.7 item).

## Commits (Area 3)

| sub-item | commit |
|---|---|
| 3.1 (+3.4/3.6 wiring) | `6f1976f` |
| 3.2 + 3.5 verification & hibernate trim fix | `aa1e93f` |
| 3.3 investigation | `cc8d56c` |
| 3.7 + report + worklog | (this commit) |

## Upstream issues drafted this Area

1. Font memory: glyph subsetting, font-face expiry, accounting crash
   (`docs/upstream/area3-font-memory.md`).
2. WebRender texture/texture-cache purge on pipeline exit (§3.5 numbers
   as reproduction data).
3. `PreferencesUpdated` does not re-apply GC parameters (`js_mem_max`
   snapshot) (§3.4).

## Conclusion

Area 3 delivers a working memory-management stack beyond Phase 2/3:
per-tab budgets with a two-step degradation ladder, system-aware budget
shrinkage, JS-heap tiers with idle-timer detection, PSI-driven reclaim,
and a 3.5× more effective hibernation trim. The comparison against
Chrome is honest: brows12 wins on raw RSS almost everywhere but the
"2× lighter per page" target is met on 1/8 pages; closing the gap
requires a release build (embedder-side, next phase) and four upstream
engine changes, all documented with reproduction data.
