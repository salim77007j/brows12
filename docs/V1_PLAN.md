# V1 Plan — From "Mostly matches Chrome" to real-world competitive

**Baseline:** v0.4.0 (`d0e3736`), 114 tests green, CI green, 11 side-by-side
Chromium comparisons committed. All five v0.4.0 mission gaps (floats, grid,
overflow clipping, `@font-face`, colspan/rowspan) are fixture-verified.

**Method (unchanged):** every fix is verified by rendering the same page in
Playwright Chromium and in brows12 at 1280×800 and comparing side by side.
No success is claimed without the comparison. One fix per commit.

**Target:** tag `v1.0.0-rc1` after the 25+ site real-world suite and
`docs/V1_FINAL_REPORT.md`.

---

## Audit findings (fresh, this session)

Re-ran the full suite (114 pass) and re-captured all six Chromium
references. Honest remaining deltas from the comparisons:

| Site | Verdict today | Dominant visible gaps |
|------|---------------|----------------------|
| example.com | Yes | — |
| Hacker News | Mostly | orange header / `bgcolor` presentational attrs; table cell padding |
| Bing results | Mostly | hidden sidebar nav painted (visibility semantics); form controls (toggles, search box); centered content column |
| rust-lang.org | Mostly | 2025 flex nav stacks vertically (flex min-content); hero grid column + "Why Rust?" section missing |
| Wikipedia (Vector-2022) | Mostly | **dark canvas** (background resolution bug); giant `h2` (heading font-size); absolute sidebar stacks over article; sticky; max-width containers ignored |
| github.com | Mostly | dropdown/mega-menu panels painted that Chromium hides; absolute panels overlap; hero background image missing |

## Root-cause hypotheses (to confirm with probes during implementation)

1. **Hidden-element semantics** — Chromium hides menus via `display:none`
   rules / `[hidden]` / `aria-hidden` / offscreen `clip`. Our selector
   matching or UA sheet misses some of these paths, so hidden panels paint.
2. **Wikipedia dark canvas** — `dark_preferred` defaults `false` and nothing
   overrides it, so the dark fill must come from a *non-media-query* rule
   winning the cascade (suspect: `background` shorthand / custom-property
   resolution on `body`, or a stylesheet served to our UA). Probe: dump the
   fetched CSS and the resolved `body` background.
3. **Giant h2** — Vector-2022 sets `h2 { font-size: 1.5rem; }` inside
   container/`@media` blocks and relies on `rem` + `min()`; suspect our UA
   heading sizes compound with author rules or rem resolution doubles.
4. **Flex nav stacking** — flex items whose min-content is computed as the
   unwrapped line width get one item per line; the *automatic minimum size*
   (`min-width:auto`) and `min-width:0` propagation through intermediate
   containers is incomplete.

---

## P0 — highest real-world impact (implement first)

| # | Item | Why it matters | Verify on |
|---|------|----------------|-----------|
| P0-1 | Hidden-element semantics: `[hidden]`, `display:none` UA rules for `template`/`datalist`/`dialog[open]`-less, `aria-hidden` paint skip, `visibility:hidden` | GitHub menus, Bing sidebar, every site with disclosure panels | GitHub, Bing |
| P0-2 | Wikipedia dark-canvas bug — probe and fix the cascade/background-resolution root cause | Wikipedia is the #1 reference site | Wikipedia |
| P0-3 | Heading font-size cascade fix (UA h1–h6 + author `rem`) | Every article page | Wikipedia |
| P0-4 | **A1 Flex min-content integration**: automatic minimum size, min-width:0 propagation, flex-basis content%, nested containers, inline-flex | Vector-2022, rust-lang.org nav, GitHub | 3 sites + fixture |
| P0-5 | **A2 position: sticky** (scroll container) + **fixed** scroll anchoring already partial | Wikipedia sidebar, GitHub header | both + fixture |
| P0-6 | **B7 calc()/min()/max()/clamp()`** — parsed but dropped to 0 today | modern CSS uses these everywhere | fixture + sites |
| P0-7 | Form controls paint: input/button/select/checkbox baseline | Bing, GitHub sign-in flows | Bing |

## P1 — styling completeness

| # | Item | Notes |
|---|------|-------|
| P1-1 | **B2 box-shadow / text-shadow** (multiple, spread, inset, blur, alpha) | cards everywhere |
| P1-2 | **B3 border-radius full**: 4 corners, elliptical, %; clip bg/content | cards, avatars |
| P1-3 | **B4 background-image full**: `url()` layers, multiple layers, size/position/repeat/origin/clip + gradients already exist | hero sections |
| P1-4 | letter-spacing / word-spacing | old known gap |
| P1-5 | margin collapsing (CSS 2.1 §8.3.1) | article typography |
| P1-6 | **B1 WOFF2**: brotli + table reconstruction (pure Rust, `brotli` crate; ttf-parser for SFNT re-assembly), font collections, multi-source @font-face | web fonts |
| P1-7 | **A3 transforms 2D complete**: skew, per-axis scale, matrix decomposition, transform-origin, nested composition (paint-only) | demos, real sites |
| P1-8 | **B9 ::before/::after** with `content` (incl. `attr()`, counters-lite) | icons, quotes |
| P1-9 | **B8 custom properties element-scoped** (replace document-global approximation) + runtime `setProperty` invalidation | theming |
| P1-10 | Presentational attributes: `bgcolor`, `width`/`height` attrs, table `cellpadding`/`cellspacing`, `border` attr | HN header |

## P2 — layout breadth

| # | Item | Notes |
|---|------|-------|
| P2-1 | **A5 CSS columns** (count/width/gap/rule, balancing) | news sites |
| P2-2 | **B5 filters** (blur/brightness/contrast/grayscale/hue/invert/saturate/sepia/drop-shadow, chains) | modern UIs |
| P2-3 | **B6 clip-path** (inset/circle/ellipse/polygon) | hero art |
| P2-4 | **A6 writing modes** (horizontal-tb/vertical-rl/vertical-lr + text-orientation) | CJK |
| P2-5 | **B10 cursor** property plumbing to the UI layer | UX |
| P2-6 | **A4 animations/transitions**: verify the existing deterministic-clock engine against Chromium frame-by-frame; fill timing-function/direction/fill-mode deltas | polish |

## P3 — JS/DOM platform (much exists per CAPABILITY_REPORT; verify + fill)

| # | Item | Notes |
|---|------|-------|
| P3-1 | C1 observers: MutationObserver granularity, ResizeObserver continuous, IntersectionObserver threshold lists | lazy-load |
| P3-2 | C3 History API: pushState/replaceState/popstate/go + location.* full surface | SPAs |
| P3-3 | C4 navigator/window: innerWidth/Height, devicePixelRatio, scrollX/Y, scrollTo/By, matchMedia | apps |
| P3-4 | C7 Web Components: customElements.define + lifecycle, shadow DOM with slots, `<template>` | YouTube-class |
| P3-5 | C2 rAF at 60 FPS from frame clock (verify), C5 storage (exists; sessionStorage session-clear fix), C6 fetch options coverage | verify + gaps |
| P3-6 | D1 Canvas 2D: gradients (real), getImageData/putImageData, clip, patterns | canvas demos |
| P3-7 | D2 WebGL2 FBOs + instancing; D3 WebGPU feasibility write-up; D4 WASM (wasmi — keep, justify); D5 WAAPI playback control | document honestly |

## P4 — rendering quality + performance

| # | Item | Notes |
|---|------|-------|
| P4-1 | E3 bilinear/bicubic image scaling (tiny-skia has quality paths — verify + use) | scaled imgs |
| P4-2 | E2 sub-pixel glyph positioning (swash gives subpixel advances — verify int truncation in display list) | text metrics |
| P4-3 | E4 HiDPI: devicePixelRatio-aware raster (render at DPR, compositor scales) | retina |
| P4-4 | E5 sRGB correctness check on gradient banding | polish |
| P4-5 | F1/F2/F3: measure first (per-stage timings + compositor stats), then implement the cheapest real win (e.g. scroll without re-layout already exists — verify 60 FPS claim, document) | evidence-based |
| P4-6 | F4 memory: document per-tab RSS on representative pages | report |

**Rejected/deferred (justified):** WebRTC (no mature Rust stack — unchanged
from v0.2 decision), Service Workers (event-loop model change — defer with
note), media playback (no demux stack — document), WebGPU render pipelines
beyond compute (keep compute; document the boundary), wasmtime swap (wasmi
fits the CI constraints; leaf change if needed later).

---

## Execution order

P0-1 → P0-2 → P0-3 → P0-4 → P0-5 → P0-6 → P0-7 → P1 (in table order) →
P2 → P3 → P4 → 25-site suite → `V1_FINAL_REPORT.md` → release build → CI
update → push → tag `v1.0.0-rc1`.

Commit after every fix. Worklog at `docs/WORKLOG.md` updated per fix.
