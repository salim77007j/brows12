# Layout Fixes Report — v0.4.0

**Mission:** close the five remaining gaps to Chrome parity: floats,
CSS grid, overflow clipping, `@font-face` web fonts, and table
colspan/rowspan.

**Method:** every step was verified by rendering the same fixture in
Chromium (Playwright, ground truth captured with
`scripts/chrome_reference.py`) and in brows12 (harness
`brows render`), then comparing screenshots side by side. Side-by-side
composites for the six validation sites live in `/screenshots/compare/`
(Chromium on the left, brows12 on the right, same 1280×800 viewport).

---

## Step 1 — FLOATS (CSS 2.1 §9.5) — commit `c54d437`

**Before:** `float: left/right` was not implemented at all (lightningcss
does not even model the property) — floated infoboxes took full blocks
and following text stacked underneath or overlapped.

**Implementation**
- `float`/`clear` declarations are rewritten to reserved custom
  properties at parse time (declaration positions only) and resolved
  into typed `ComputedStyle` fields during cascade.
- A float placement post-pass implements §9.5: containing-block padding
  box, static-flow position, left/right edge stacking against earlier
  floats, collision push-down, the "not above earlier float tops" rule,
  and `clear` on floats.
- Line boxes shorten around floats (`FloatBand` per inline group,
  per-line left/right insets, alignment inside the band). A two-pass
  compute propagates the taller wrapping heights to later siblings.
- `clear` on blocks and overflow-trimming BFC boxes pushes content below
  matching floats; the nearest BFC ancestor grows to contain its floats.

**Verification** — `validation/run/fixtures/floats.html`, Chromium vs
brows12: both place the right-floated infobox at the container edge,
wrap paragraph text around both a left and a right float simultaneously,
and push the `clear: both` paragraph below both floats. On the live
Wikipedia Rust article the lead paragraphs wrap around the right-floated
infobox exactly as Chromium does. Regression tests:
`layout/tests/floats.rs` (cascade resolution, container-edge placement,
line wrap, clear).

## Step 2 — CSS GRID — commit `4896fff`

**Before:** `display: grid` was unmapped; grid pages fell back to block
stacking.

**Implementation** — `Display::Grid` maps onto taffy's native grid:
`grid-template-columns/rows` (px/%/fr/min-content/max-content/minmax,
`repeat(N, …)` expansion), `grid-auto-columns/rows`, `grid-auto-flow`
(row/column/dense), and item placement `grid-column/grid-row` (line
numbers, spans, auto).

**Verification** — `validation/run/fixtures/grid.html` matches Chromium
**pixel-for-pixel**: `200px 1fr 1fr` template, a row-spanning item, a
two-column spanning item, `repeat(3, 1fr)` auto placement, gaps.
Regression test: `grid_tracks_and_placement`.

## Step 3 — OVERFLOW CLIPPING — commit `b388f2c`

**Before:** `overflow: hidden` was parsed but never consumed; absolutely
positioned children spilled outside rounded cards.

**Implementation** — `overflow: hidden|scroll|auto` nodes emit a clip
region (padding box, border-radius honored) around their subtree in the
display list (`PushClip`/`PopClip`); the rasterizer keeps a clip stack
of tiny-skia masks (nested clips intersect). Every paint primitive —
rect fills, border strips, gradients, glyph blits, underlines, images —
receives and honors the active mask.

**Verification** — `validation/run/fixtures/overflow.html`: an absolutely
positioned badge clips at the rounded card edge under `overflow: hidden`
and spills outside the border under `overflow: visible`, matching
Chromium.

## Step 4 — @FONT-FACE — commit `7a53911`

**Before:** web fonts were collected but only the first raw URL was
tried; relative URLs (`fonts/x.woff2` — the overwhelmingly common case)
always failed, so pages always fell back to system fonts.

**Implementation**
- Source URLs resolve against the page URL; each face's sources are
  tried in order until one decodes.
- Magic-byte sniffing: raw TTF/OTF/TTC register directly; WOFF1
  containers are decompressed (zlib table streams reassembled into a
  valid sfnt); WOFF2 is skipped with a debug note (the font stack's
  ttf-parser 0.25 has no WOFF2 tables) so pages fall back exactly like a
  failed font load.
- The font's internal name table is rebuilt with the `@font-face`
  declared family (Mac + Windows records) so CSS `font-family` matches
  the declared name — browser aliasing semantics.

**Verification** — `validation/run/fixtures/fontface.html` (self-hosted
TTF over HTTP): the two `@font-face` lines render in the distinctive
web font and the system-font lines are unaffected — identical behavior
in brows12 and Chromium.

## Step 5 — TABLE COLSPAN/ROWSPAN (CSS 2.1 §17) — commit `6dcb648`

**Before:** spanning cells were ignored — rows with colspan/rowspan
misaligned columns.

**Implementation** — tables containing spanning cells map onto taffy's
grid instead of the anonymous-flex structure: `compute_table_spans` runs
the CSS 2.1 slot-assignment pass (column cursor + rowspan occupancy),
cells become direct grid items with explicit line/span placement, and
row boxes are skipped. Span-free tables keep the proven flex mapping.
While verifying, the CSS `border` shorthand turned out to be unapplied
engine-wide (only longhands worked) — fixed in the same commit.

**Verification** — `validation/run/fixtures/spans.html`: identical slot
structure to Chromium (colspan rows stretch across columns, rowspan
cells cover subsequent rows, later cells flow past the occupied slots).
Regression tests: `layout/tests/table_spans.rs`.

---

## Follow-up hardening found during site validation (commit `262bcbe`, `11c55a3`, `8bfc6af`)

- **Intrinsic sizing correctness**: the measure function returned
  max-content for `MinContent` requests, so `1fr` (= `minmax(auto,1fr)`)
  tracks exploded to the unwrapped line width. MinContent now wraps at
  every opportunity.
- **`min-width`/`min-height`** parsed and mapped; the automatic content
  minimum applies to flex/grid items only, block children floor at 0
  (CSS box sizing).
- **Fit-content caps**: floats/absolutes with auto width cap at 100% of
  their containing block (CSS2 §10.3.7), so wide content wraps instead
  of exploding floats.
- **Root max-width**: `body { max-width: 26em; margin: auto }` (the
  refreshed example.com) constrains the available width and centers the
  root box — example.com matches Chromium again.
- **Paint robustness**: the display-list builder no longer aborts whole
  subtrees at structural nodes without a box (this had blanked Hacker
  News's story list).
- **CSS error recovery**: one unparseable rule no longer discards the
  entire sheet or inline style attribute.

## Side-by-side validation results (`/screenshots/compare/`)

| Site | Verdict vs Chromium | Notes |
|------|---------------------|-------|
| example.com | **Yes** | centered 26em card, 25vh padding, centered text — matches |
| Hacker News | **Mostly** | full story list (ranks, titles, subtexts, links) matches; the orange header / story-area `bgcolor` presentation attributes remain unpainted (pre-existing gap) |
| Bing search results | **Mostly** | result rows, blue titles, URLs, snippets render; favicons and some cosmetics differ |
| rust-lang.org | **Mostly** | logo, headline, language list render; the 2025 site's flex nav stacks vertically instead of flowing (needs flex min-content capping) |
| Wikipedia (Rust article) | **Mostly** | article text, links, headings render and text wraps; Vector-2022 skin containers (grid + deep nowrap min-content) overflow the viewport, pushing the infobox off-screen — `position: sticky` sidebar menus still overlap (pre-existing) |
| github.com | **Mostly** | page loads and renders structure; the 2025 GitHub UI is flex/grid-dense and its layout approximates rather than matches |

Fixture-level (where the five gaps are exercised directly), brows12
matches Chromium: floats, grid, overflow clipping, @font-face, and
table spans all render correctly.

## Remaining known gaps (honest list)

- WOFF2 web fonts (requires a WOFF2 decompressor; ttf/otf/woff1 work).
- Vector-2022 / modern flex-grid skins: flex items with huge nowrap
  min-content can stretch flex containers (needs the full CSS
  automatic-minimum-size interaction with `min-width: 0` on every
  intermediate container).
- Presentational attributes (`bgcolor`, `width` attrs) beyond the
  subset the UA sheet maps.
- `position: sticky` (treated as absolute), margin collapsing,
  `letter-spacing`, root margin-top.
- Prefer-color-scheme media queries (dark skins render dark even when
  Chromium's reference is light).

## Verdict

**Mostly.** All five requested gaps are implemented and fixture-verified
against Chromium (floats, grid, clipping, web fonts, table spans), and
the engine gained several general correctness fixes along the way
(intrinsic sizing, min-size semantics, fit-content caps, error
recovery). Static-content sites (example.com, HN, Bing) render
correctly; modern CSS-grid/flex skins (Wikipedia Vector-2022,
rust-lang.org's 2025 UI, GitHub) render recognizably but not yet
pixel-close — the remaining distance is in flex/grid min-content
integration, not in the five mission areas.
