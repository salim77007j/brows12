# Phase 4 Area 3.3 — Font memory: what exists, what is missing

Embedder: brows12 (Servo 0.6.0 crates.io pin + local `patched/` overlays for
stylo / servo-layout / servo-canvas / servo-config / script-bindings).

## What the engine already does (verified in code, servo-fonts 0.6.0 +
  servo-paint 0.6.0 + patched/servo-layout)

1. **Cross-tab sharing is inherent.** `FontStore` / `SystemFontService`
   templates and WebRender `FontKey`/`FontInstanceKey` resources are
   process-global: two tabs rendering the same webfont share one copy of
   the font data. No per-tab duplication exists to fix.
2. **Font resource retirement exists end-to-end.** After display-list
   rebuilds, layout collects the font keys no longer referenced and calls
   `PaintApi::remove_unused_font_resources(webview_id, keys, instance_keys)`
   (`patched/servo-layout/layout_impl.rs`), which forwards
   `PaintMessage::RemoveUnusedFontResources` to WebRender — the WR-side
   texture-cache entries for those fonts are dropped.
3. **Hibernation removes the referents.** Dropping a hibernated tab's
   WebView tears down its pipeline; its display lists stop referencing
   fonts, so nothing pins new WR font allocations.

## What is missing (upstream gaps; embedder cannot fix)

1. **Whole-file font caching, no glyph subsetting.** `servo-fonts` caches
   the complete font blob per template (`Arc<FontData>`); large CJK
   families (10-30 MB) are pinned in full even when a page renders a few
   hundred glyphs. Subset-on-use would cut the dominant cost.
2. **No expiry / unload policy.** `FontStore` has no last-used timestamp
   or N-second unload; webfonts of long-idle-but-live tabs stay resident
   forever. (Hibernation — dropping the whole pipeline — is our only
   lever, and brows12 already applies it aggressively.)
3. **Accounting is broken.** `Servo::create_memory_report` crashes in
   `SystemFontService` (known Phase 2 bug, upstream issue drafted in
   Area 1.7), so font-cache size cannot even be measured from the
   embedder. Until that lands, font memory is only visible as part of
   process RSS.

## Downstream impact path

Font memory is bounded in brows12 by the hibernation policy (3.1/3.4):
a tab past its suspend delay has its whole pipeline — including its
font references — dropped, and WR retires the resources. What remains
exposed is long-lived foreground misuse (hundreds of families), which is
an edge case we accept for now.
