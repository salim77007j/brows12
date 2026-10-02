# Rendering Quality Upgrade — Plan & Status

**Goal:** pages render like a major browser — correct text, correct layout,
real images — instead of a jumbled text pile.

**Method:** every change is verified by rendering live sites with the engine's
harness (`brows render --url …`) and comparing against headless-Chromium
ground truth captured at the same viewport (`validation/run/reference/`).

## Root causes found & fixed (chronological)

| # | Bug | Effect on pages | Fix |
|---|-----|-----------------|-----|
| 1 | swash glyph placement applied **twice** (`blit_swash_image` re-added `placement.left/top` after the caller had already applied them) | Every glyph's bitmap top sat **on** the baseline → letters bounced up/down and sideways: the "reversed and jumbled text" on every page | Apply swash placement exactly once (`rendering/src/raster.rs`) |
| 2 | No inline layout — every text node was its own block, `<a>`/`<b>` became full-width blocks | Text could not flow across links; nav bars shattered into stacked boxes; links wrapped onto own lines | Full inline layout engine (`layout/src/inline.rs`): per-span shaping, greedy word wrap across styled runs, line-box assembly, text-align, per-segment paint (bg/underline/strike) |
| 3 | CSS whitespace collapsing dropped **leading** spaces of chunks | "Rust**and bold text" — spaces after inline elements vanished | `collapse_ws` keeps single boundary spaces; line-start removal happens at wrap |
| 4 | BiDi glyph clusters panic | Arabic/Hebrew pages crashed the renderer | min/max byte ranges per token, absolute advances |
| 5 | `Len::Percent` stored a **fraction** (25% = 0.25) but every consumer divided by 100 again | **All CSS percentages computed at 1/100th**: `top:N%`/`left:N%`/width%/font-size% were effectively zero/near-zero — the #1 layout-collapse bug | Consistent fraction semantics everywhere (`taffy` mapping, insets, font-size, margins, animation round-trip) |
| 6 | taffy does not resolve percentage `top`/`bottom` insets on absolute children; auto-inset absolutes stacked at origin | Wikipedia's `lang1..lang10` (top:N% left/right:60%) all piled up | Own absolute resolver: percentage insets against the containing block's padding box; **static-flow position** for auto insets (CSS 2.1 §10.3.7) |
| 7 | Tables not implemented | Hacker News / Wikipedia articles unreadable | `display: table/row/cell` → anonymous flex structures (table = column flex, row = row flex, cell = content-sized shrinking item); UA stylesheet updated |
| 8 | Underline drawn *above* the baseline | Links looked struck-through | Underline below baseline, line-through at x-height |

## Capabilities added

- **Inline layout engine** — cross-element line flow, mixed font sizes,
  inline backgrounds, `<br>` hard breaks, BiDi-safe shaping
- **CSS tables** — content-driven columns like Chrome for content tables
- **Real absolute positioning** — percentage insets + static position
- **z-index paint order** — stable stacking-context buckets
- **SVG images** via resvg (logos/icons decode and scale)
- **CSS gradients** — linear (angle/keywords/corners) + radial, per-pixel
  rasterizer with stop interpolation
- **text-transform** — uppercase / lowercase / capitalize

## Verified results (fresh evidence, same window as Chrome reference)

| Site | Before | After |
|------|--------|-------|
| example.com | jumbled glyphs, invisible heading | **matches Chrome** (2025 centered-card layout incl. `max-width:26em`, `margin:auto`, `25vh` padding) |
| www.wikipedia.org (portal) | overlapping language pile | **10 language boxes scattered around the globe exactly like Chrome**, CJK/Arabic/Cyrillic counts render |
| en.wikipedia.org article | text pile + missing images | big clean h1, sidebar, **SVG gear logo renders**, infobox table renders; sidebar sticky menus still overlap (known gap) |
| news.ycombinator.com | text pile | **like Chrome**: rank column, left titles, subtext; missing orange header bg (table `bgcolor` attr) |
| Bing search results | barely readable | **real search results page**: favicons, URL rows, blue titles, snippets |
| rust-lang.org | nav stacked vertically | flows: language list inline, giant serif headline, SVG logo |

## Known gaps (honest list)

- **Floats** not implemented (Wikipedia infoboxes take a full block instead
  of wrapping text around them)
- **CSS grid** not mapped (taffy supports it; property parsing pending)
- **`overflow: hidden` clipping** not implemented (absolute elements can
  spill outside rounded headers)
- **`@font-face` / web fonts** not fetched — pages fall back to system fonts
- **letter-spacing / word-spacing** ignored
- **colspan/rowspan** not supported in the table mapping
- Wikipedia article **sticky sidebar menus** overlap (sticky treated as absolute)
- github.com returns 403 "Forbidden" from this datacenter IP (server-side
  bot blocking, not a rendering issue)

## Verification workflow

```sh
cargo build -p brows12-harness
./target/debug/brows render --url https://example.com --png out.png
# Chrome ground truth:
python3 scripts/chrome_reference.py   # → validation/run/reference/*.png
# Full UI validation (10 steps, event-driven, under Xvfb):
XKB_LIB_DIR=$PWD/validation/run/xkb bash validation/run_validation.sh
```
