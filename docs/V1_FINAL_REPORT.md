# V1 Final Report — brows12 v1.0.0-rc1

**Mission:** from "Mostly matches Chrome" to real-world competitive.
Baseline: v0.4.0 (114 tests). This report covers the v1 sprint: audit,
P0 fixes, Group A/B rendering work, the 25+ site suite, and an honest
verdict. Companion: `docs/V1_PLAN.md` (plan), `docs/WORKLOG.md`
(per-fix log with evidence).

---

## 1. Executive summary

The sprint began with a full re-audit and re-verification (fresh clone,
origin/main @ dd77f21, 115 tests green). Five P0 fixes already landed in
prior sessions (hidden elements, Wikipedia dark canvas, heading cascade,
sticky/fixed, calc(), form controls) were verified; the missing P0-4
(flex min-content integration) was implemented and, while verifying it,
two deeper root causes were found and fixed: an eager percentage
resolution that collapsed Wikipedia to a blank header cavity, and a
default User-Agent that made Wikipedia serve a broken page variant.
Styling completeness grew by opacity:0 subtree skipping and a full
box-shadow/text-shadow pipeline with real blur. Page images now load
concurrently.

The 25+ site suite shows major gains — example.com and reddit are pixel-
close, HN/docs.rs/arxiv/svelte/threejs/mozilla/MDN/google render
recognizably, wikipedia/rust-lang.org/bing/stackoverflow/react render
with layout deltas — but JS-heavy SPAs (crates.io, vuejs.org) and
art-heavy pages still fail. Honest estimate: **~75% of the suite renders
"correctly or mostly" today**, against the 90% goal.

## 2. Per-fix before/after

| Fix | Before | After | Evidence |
|-----|--------|-------|----------|
| Compat User-Agent | Wikipedia served broken no-JS variant (no RLCONF) to `Brows12/0.2.0` | standard Vector-2022 page served | fetch probes, `net` unit test |
| A1 flex min-content | item grow/shrink/basis only applied to flex containers; `flex: 1` no-op on block items; nav rows stacked | item properties applied unconditionally; taffy distributes like Chrome | 5 layout tests; `flex_fixture_compare.png` (6/6 match); rust-lang.org nav horizontal |
| A1 inline-flex | `display: inline-flex` blockified | InlineFlex variant: shrink-to-fit atomic box | flex fixture case 4 |
| A1 flex-basis | never parsed (grow/shrink only) | FlexBasis enum + taffy mapping, symbolic percents | fixture case 5 (60px/30%/80px matches Chrome arithmetic) |
| Symbolic percent resolution | `width:100%` on abs-positioned dropdown checkbox = 1192×1192 → 1220px header, page "blank" | percents resolve against the real containing block; checkbox 32×32, header 50px | `wikipedia_rust_compare.png` |
| opacity:0 | hidden dropdown checkboxes painted as black squares | subtree paint skip (group semantics) | Wikipedia ToC clean |
| B2 box-shadow | unsupported | multi-layer, spread, inset (approx), blur via 3-pass box blur | `shadows_fixture_compare.png`; pixel samples within ~5 gray levels of Chrome |
| B2 text-shadow | unsupported | glyph-silhouette blur layers beneath ink | fixture glow/heading cases |
| Concurrent images | 24 serial fetches ≈ 29s before paint | join_all over the shared client | Wikipedia same render, fetch stage parallel |

Also verified from earlier P0 work: hidden-element semantics, Wikipedia
dark-canvas (scoped design tokens), em font-size cascade,
sticky/fixed scroll correctness, calc()/min()/max()/clamp(), form
control paint, presentational attributes (HN orange header renders).

## 3. Site suite verdicts (screenshots/v1-final/)

Verdict scale: Yes / Mostly / No (No = unusable). Rendered at 1280×800.

| Site | Verdict | Notes |
|------|---------|-------|
| example.com | **Yes** | matches; minor spacing |
| reddit (bot wall) | **Yes** | same interstitial as Chrome |
| hacker_news | Mostly+ | orange header + table listing close; minor cell padding |
| docs_rs | Mostly+ | header/search/list match |
| arxiv | Mostly+ | listing + sections match; lighter styling |
| svelte_dev | Mostly+ | hero + logo image + nav close |
| threejs_org | Mostly | structure ok; demo grid images missing |
| mozilla_org | Mostly | wordmark + hero + section list; spacing deltas |
| mdn_fetch | Mostly | full article text, close layout |
| google_com | Mostly | logo + links; search box styling light |
| bing_search | Mostly | results text/layout; some overlap |
| stackoverflow | Mostly | interstitial parity; layout deltas |
| react_dev | Mostly | header/hero/CTA render |
| cloudflare | Mostly- | orange hero renders; overlapping absolute boxes |
| stripe | Mostly- | text renders; gradient hero art missing |
| linear_app | Mostly- | nav + headline; dark scheme not applied |
| wikipedia_rust | Mostly | header/ToC/title/tabs; infobox image below fold; body spacing |
| wikipedia_main | Mostly- | ToC + article list; layout spacing off |
| youtube | Mostly- | skeleton card grid renders; no video thumbs |
| github_home | No+ | dark canvas + fragments; hero art/panels off |
| vercel | No+ | boxes/overlaps |
| duckduckgo | No | search bar fragment only (JS-gated page) |
| crates_io | No | header only (SPA hydrate) |
| vuejs_org | No | near-blank (SPA) |
| tailwindcss | No | render timed out (150s cap) |
| amazon | No | render timed out / bot wall |

Totals: 2 Yes, 12 Mostly(±), 8 No(±), 2 unrendered → ~75% at Mostly- or
better; the 90% target is not yet met.

## 4. Honest capability matrix

| Area | State |
|------|-------|
| HTML/CSS parsing, cascade, error recovery | Yes |
| Block/inline layout, floats, tables (incl. spans), grid, flex | Yes (flex item sizing + basis + auto-min now spec-aligned) |
| inline-flex / inline-block | Yes (v1 approximations documented) |
| position: static/relative/absolute/fixed/sticky | Yes (sticky/fixed paint-time constraints) |
| calc()/min()/max()/clamp(), custom properties (document-global scope) | Yes |
| Percentages against real containing blocks | Yes (symbolic through taffy) |
| box-shadow / text-shadow (blur, spread, multi, inset-approx) | Yes |
| border-radius (single radius; clips + rounded paint) | Partial — 4-corner/elliptical/% not modelled |
| background gradients | Yes (linear/radial) |
| background-image url() layers, size/position/repeat | Missing |
| @font-face (TTF/OTF/WOFF1) | Yes; WOFF2 Missing |
| letter/word-spacing, ::before/::after content | Missing |
| CSS transforms, transitions/@keyframes (deterministic clock) | Partial (v0.2 engine; not re-verified frame-by-frame this sprint) |
| CSS columns, writing modes, filters, clip-path | Missing |
| opacity | Partial — 0 handled (skip), grouped fractional blending Missing |
| JS: QuickJS-ng, ES modules, fetch/XHR/WS, observers, storage, IDB, Canvas2D, WebGL2 surface, WebGPU compute, WAAPI subset | Yes (see CAPABILITY_REPORT) |
| WebGPU render pipelines, Service Workers, media playback, WebRTC | Missing (documented decisions) |
| Compositor: wgpu layer compositing + CPU fallback | Yes |
| Performance | Partial — concurrent images; ~10–80s wall per page remains (CPU-bound style/layout two-pass) |

## 5. Performance numbers (this container, dev build)

- example.com 3.4s · google 3.6s · bing 3.6s · HN 6.9s · arxiv 6.5s ·
  MDN 5.9s · docs.rs 8.3s · wikipedia_main 9.6s · crates_io 10.7s ·
  react_dev 11.5s · svelte 14.2s · threejs 13.9s · wikipedia_rust 28.6s ·
  github 50.7s · cloudflare 62.5s · vercel 82.8s · youtube 78.0s
- Bottleneck is CPU-bound style + two-pass layout on large DOMs, not
  network (image fetches now parallel). Group F (incremental layout,
  memoized cascade) is the lever for 10×.

## 6. Verdict

**Is brows12 competitive with Chrome today? Mostly.** It is no longer
fragile on mainstream editorial/documentation sites: Wikipedia, HN,
docs.rs, arxiv, MDN, rust-lang.org, svelte.dev, mozilla render
recognizably with real web fonts, flex/grid, shadows and sticky
behavior. But it does not yet hit the 90% bar across this suite.

**Correct-rendering estimate: ~75%** of the 25-site suite at Mostly- or
better (2 fully Yes).

**Top 3 remaining gaps (by impact):**
1. **CPU cost of style/layout on big DOMs** — 10–80s page loads starve
   interactive use; incremental layout + memoized cascade is the single
   highest-impact investment.
2. **background-image url() layers + WOFF2** — hero art and brand
   typography are the most visible deltas on real marketing pages
   (stripe, github, cloudflare).
3. **JS-heavy SPA hydration** — crates.io/vuejs/duckduckgo depend on
   hydration-time DOM APIs; expand rAF/observer fidelity and MutationObserver
   granularity so SPA shells render content instead of skeletons.

## 7. Recommendations for v1.0+

1. Group F first: per-stage timing instrumentation in the harness,
   memoized cascade (dirty-node recompute), single-pass layout for
   static pages, then lazy paint culling.
2. background-image (fetch+decode reuse of the img pipeline, then
   size/position/repeat) — unlocks the largest visual class.
3. WOFF2 via brotli + ttf-parser table reassembly; the font stack
   already handles @font-face source fallback so it is a leaf change.
4. Four-corner border-radius by extending the existing rounded-path
   helper (r → per-corner radii) across fill/border/clip/shadow shapes.
5. Keep the UA compat string; revisit only with a fingerprinting
   opt-out.

Deliverables: 25 comparisons in `screenshots/v1-final/`, per-fix
evidence in `docs/WORKLOG.md`, plan in `docs/V1_PLAN.md`, tag
`v1.0.0-rc1`.
