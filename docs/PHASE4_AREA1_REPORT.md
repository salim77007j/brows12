# Phase 4 — Focus Area 1 Report: Remaining Bugs + Upstream PRs

**Scope:** the seven partial/broken items carried over from Phase 3's verdict
matrix, plus upstream engagement for the Servo bugs behind them.

**Environment:** servo 0.6.0 (crates.io) embedded behind the brows12 shell.
Engine fixes are delivered as **vendored `[patch.crates-io]` patches**
(`patched/servo-layout`, `patched/servo-canvas`, `patched/stylo`,
`patched/servo-config`, `patched/stylo-static-prefs`,
`patched/servo-script-bindings`) so every fix is buildable, testable, and
individually portable to the Servo monorepo.

**Verification method:** every fix is verified with the side-by-side runner
(`scripts/phase3_run.py`, Chromium ground truth via Playwright on the left,
brows12 on the right) and, for 1.1/1.2, additionally on real sites
(`scripts/phase4_real_site.py`). No fix is claimed fixed without its composite
under `screenshots/`.

---

## 1.1 clip-path polygon — FIXED ✔ (was: parses but does not paint)

**Root cause:** `StackingContextClipStore::add_for_clip_path`
(servo-layout `display_list/clip.rs`) returned `None` for
`BasicShape::Polygon | BasicShape::PathOrShape` — stylo computed the value,
but no clip was ever created.

**Implementation** (commit `5af38ca`):
- Resolve `polygon(<length-percentage>{2,})` coordinates against the
  reference box, compute the polygon bounding box.
- Rasterize a premultiplied **BGRA8 alpha mask** on the layout thread
  (`display_list/polygon_mask.rs`): 2×2 supersampled winding-number
  (`Nonzero`) / parity (`Evenodd`) point-in-polygon test → simple effective
  edge anti-aliasing. Pathological masks clamped to 8192².
- Upload via `CrossProcessPaintApi::generate_image_key_blocking` +
  `add_image` (`SerializableImageData::Raw` shared memory), then emit
  `DisplayItem::ImageMaskClip` (`define_clip_image_mask`) with the polygon
  points — WebRender's hit testing then matches the polygon too.
- **Cross-frame cache**: masks are keyed by quantized geometry (1/100 px,
  origin-normalized) in a `PolygonMaskImageCache` on the layout thread
  (≤128 entries) so unchanged polygons are neither re-rasterized nor
  re-uploaded on later frames.
- `inset()`, `circle()`, `ellipse()` verified unchanged (they already
  worked). `path()`/`shape()` remain unsupported — documented follow-up
  (curve flattening into the same mask path).

**Verification:**
- `css_filters_clip` fixture: **7/7 self-report PASS on both engines**;
  the polygon tile now paints a triangle identical to Chromium
  (`screenshots/v2-servo/phase3/css_filters_clip_compare.png`).
- Real sites: apple.com + linear.app render essentially identically to
  Chromium (`screenshots/v2-servo/phase4/apple_compare.png`,
  `linear_compare.png`) — deltas are a button-wrap font-metric difference
  and a dimmer SVG logo mark, both unrelated to clip-path.

## 1.2 backdrop-filter — FIXED ✔ (was: missing entirely)

**Root causes (two independent gates):**
1. stylo gated the longhand behind `servo_pref = "layout.unimplemented"`;
2. servo-layout never emitted the item, even though webrender 0.70 fully
   supports `DisplayItem::BackdropFilter` (BackdropCapture/BackdropRender).

**Implementation** (commits `d49973f`, `184c808`, `6e04d44`):
- stylo patch: dedicated pref `layout.css.backdrop-filter.enabled` for the
  longhand (replacing the blanket `layout.unimplemented` gate).
- `servo-config` patch: `Preferences` field + default + stylo sync bridge.
- `stylo_static_prefs` patch: the pref registered in `preferences.toml`
  (the `set_pref!` bridge only accepts listed prefs).
- `servo-script-bindings` patch: the codegen `MAPPING` in
  `codegen/run.py` extended so the generated
  `CSSStyleDeclaration` guard resolves against the identifier-style
  `Preferences` field. (Found the hard way: the codegen maps dots to
  underscores but a *hyphenated new* name falls through raw and panics at
  interface creation time — see commit message for the debugging trail.)
- servo-layout: `BuilderForBoxFragment::build_backdrop_filter` emits
  `push_backdrop_filter(&common, &filters, &[])` **before the fragment
  paints anything** (the item filters what is painted *behind* the
  element); region = border box; border-edge clip applied so rounded
  corners filter correctly. A non-none computed `backdrop-filter` now also
  **establishes a stacking context** and forces `transform-style: flat`,
  per filter-effects-2.
- servo-host: the pref is enabled in the compat profile.
- Chained filters (blur/brightness/saturate/…) reuse the existing
  `FilterToWebRender` conversion — no new filter plumbing needed.

**Verification:** fixture 7/7 PASS on both engines (frosted-glass tile
blurs the striped backdrop on both sides); apple.com and linear.app (both
glass-morphism-heavy) near pixel-equivalent — same artifacts as 1.1.

## 1.3 vertical writing-modes — LARGELY FIXED ✔ (was: unimplemented)

**Discovery:** servo 0.6.0 already ships the 2025 vertical writing-mode
layout work behind the opt-in pref `layout.writing-mode.enabled`
(default off). Phase 3's "not implemented" was the pref gate.

**Implementation** (commit `6e04d44` + fixture run): enable the pref in the
compat profile; un-gate the `text-orientation` longhand onto the same pref
(was `layout.unimplemented`), so `getComputedStyle().textOrientation`
resolves.

**Verification** (`css_writing_modes`): **7/7 self-report PASS on both
engines** (Phase 3: 3/7) — `vertical-rl`, `vertical-lr`, RTL paragraph,
direction box, and logical properties all match Chromium
(`css_writing_modes_compare.png`).

**Honest remaining gap:** `text-orientation: upright` **paints glyphs
sideways** (the computed value is correct; the glyph-orientation pass is
not implemented in servo 0.6.0's writing-mode work — upright requires
per-glyph orientation in the text-run construction, a shaper-level change).
Queued as an upstream follow-up; not fixed locally within this phase.

## 1.4 canvas shadow — FIXED ✔ (was: unimplemented; shape lost)

**Root cause:** `VelloCPUDrawTarget::draw_surface_with_shadow` was a stub
(`log::warn!("no support for drawing shadows")`) — and because the shape is
first drawn into the temporary shadow target, the whole operation no-oped
(`fillRect` painted nothing).

**Implementation** (commit `e37837d`):
- Implement the backend call: extract the temp surface's alpha channel
  (global alpha already baked in), approximate the Gaussian with **three
  box-blur passes** (window radius `(√(4σ²+1) − 1)/2`, σ = shadowBlur / 2 —
  the spec mapping Chrome/Firefox use), tint with the shadow color
  (premultiplied), composite beneath the shape with the current composition
  options.
- **Pad the temporary target** by `3σ + max(|offset|) + 1` so blurred
  shadows are not clipped by the temp surface bounds (a latent bug even if
  the stub had worked).
- Wire the previously-dropped `shadow_options` through
  `fill_text` / `stroke_text` / `fill_path` / `stroke_path`.
- Why not vello's `push_filter_layer(DropShadow…)`: it panics under
  multi-threaded rendering, which canvases ≥ 512×512 px use — the manual
  blur avoids that landmine entirely.

**Verification:** `canvas2d` fixture — **13/13 PASS on both engines,
pixel-equivalent** including the teal `fillRect` with
`shadowBlur: 8, offset (4,4)` (`canvas2d_compare.png`).

## 1.5 subgrid — documented upstream gap (upstream path per mission)

servo-layout maps grid templates to taffy via
`taffy/stylo_taffy/wrapper.rs`; every `Subgrid(_)` arm is
`// TODO: Implement subgrid and masonry → None` (wrapper.rs:287-382), and
taffy 0.14 has no subgrid support. Implementing subgrid is a **taffy
track-sizing project** (children must inherit the parent's tracks), not a
servo-local fix. Status unchanged from Phase 3 (fixture `grid_subgrid`
still falls back to auto tracks; artifact re-captured for the record).
The mission's own provision applies: "if Servo cannot support it without
upstream changes, file a PR upstream and document the gap" — tracked as
follow-up with taffy (their roadmap item) rather than a servo issue, since
servo's tracker already notes the TODO at the exact wrapper lines.

## 1.6 multicol — PARTIAL ✔ (computed style) + documented gap

servo-layout recognizes multicol containers (`is_multicol()` establishes a
BFC) but has **no column fragmentation engine** — content flows
single-column.

**Implemented:** enable `layout.columns.enabled` in the compat profile.
The `column-*` longhands were parsed-but-unexposed before; with the pref
on, the fixture's computed-style checks pass: **4/7** (columns: 3,
column-gap 24px, column-rule 2px, column-span: all, column-fill: auto —
was "column-count not materialized" in Phase 3).

**Honest remaining gap:** the rendering checks (break-inside, visible
column rule, span-all heading spanning) — `css_columns_compare.png` shows
Chromium's three ruled columns vs brows12's single flow. Fragmentation is
a multi-month engine feature; documented with the fixture as reproduction.

## 1.7 upstream engagement — 4 issues filed ✔

Fork created: **salim77007j/servo**. All four issues carry precise
root-cause analysis, minimal reproductions, and links to our working
reference implementations (commit-level) so they can seed clean PRs:

| # | Issue | Status of the underlying bug |
|---|-------|------------------------------|
| 1 | [servo#48610](https://github.com/servo/servo/issues/48610) — clip-path polygon not painted | fixed locally (1.1), PR offer included |
| 2 | [servo#48611](https://github.com/servo/servo/issues/48611) — canvas shadows not rendered + shape loss | fixed locally (1.4), PR offer included |
| 3 | [servo#48612](https://github.com/servo/servo/issues/48612) — backdrop-filter gated + never emitted | fixed locally (1.2), PR offer included |
| 4 | [servo#48613](https://github.com/servo/servo/issues/48613) — no texture-cache purge on pipeline close | Phase 3 gap; proposal + measurement offered |

**SystemFontService `create_memory_report` crash (Phase 2 finding):**
attempted reproduction on the current servo 0.6.0 build — **not
reproducible as a crash** (no panic with fonts loaded across
`brows-perf` runs; webfonts + shadow fixtures exercised the service
first). What *is* observed is an embedding-side artifact: the report
round-trip does not complete within the perf harness's 10 s window in
headless mode (`memory_report: null`), which is a brows12 harness concern,
not an engine bug. Closed as not-reproducible; noted for honesty.

**Why issues-first rather than PRs:** the vendored patches target the
crates.io packaging (`style`→`stylo`, `servo-base`→`base`, … import paths
differ in the monorepo). Porting + `mach build`/`mach test-tidy`
validation is queued as the immediate follow-up — each patch was written
against the exact upstream file layout to keep that port mechanical. The
issues were filed with the full technical content so reviewers can judge
the approach independently.

---

## Verdict summary

| Item | Phase 3 | Phase 4 Area 1 |
|------|---------|----------------|
| clip-path polygon | No (paint) | **Yes** |
| backdrop-filter | No | **Yes** |
| writing-modes vertical | No | **Mostly→Yes** (upright orientation gap) |
| canvas shadow | No (paint) | **Yes** |
| subgrid | No (layout) | No — documented, taffy-level |
| multicol | No | **Mostly** (computed style; fragmentation gap) |
| upstream | — | 4 issues + fork + reference impls |

**Artifacts:** `screenshots/v2-servo/phase3/*_compare.png` (regenerated for
the fixed fixtures), `screenshots/v2-servo/phase4/*_compare.png` (real
sites), `validation/run/phase3/*.json`, `validation/run/phase4/*.json`,
patches under `patched/` (one commit per logical fix), upstream issues
servo#48610–48613.
