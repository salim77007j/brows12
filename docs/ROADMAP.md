# Roadmap

Honest gap list for v0.1, ordered by user-visible impact.

## Layout & rendering (the big three)

1. **Inline flow** — mixed inline boxes (text runs with inline elements,
   images in lines) currently degrade to block stacking. Plan: inline
   formatting contexts on top of taffy leaves + cosmic-text line boxes.
2. **Positioning** — absolute/fixed/sticky (the CSS values parse and store
   today; the layout engine ignores them).
3. **Tables & floats** — legacy but everywhere; table support lands via a
   dedicated taffy-like grid pass.

## CSS

- `@media` query evaluation (parsed today, not evaluated — add a viewport
  predicate pass), `@supports`, `@keyframes` + the animation model.
- Custom properties (var()) with inheritance.
- Pseudo-elements `::before/::after` with generated content.
- Full Stylo-grade selector engine (swap-in behind `brows12-css::matcher`).
- Presentational attribute fallbacks (width/height on table cells, etc).

## JavaScript & DOM

- Event loop upgrades: message channels, MutationObserver, IntersectionObserver.
- ES modules with a URL-based loader (`rquickjs` loader feature is wired).
- Web Workers (one QuickJS runtime per worker, channel transport — the
  fetch bridge already speaks plain data across threads).
- Full event objects (bubbling, capture phases) over the existing listener
  registries.

## Networking

- Alt-Svc / HTTPS-record based transparent HTTP/3 upgrade (the h3 client
  exists behind `--features http3`).
- DoH-integrated connection pool (engine DNS feeding the connector).
- QUIC connection migration, 0-RTT.
- Service worker storage layer (needs the event loop upgrades first).

## Rendering

- Scrollable overflow + compositor scrolling (the layout result already
  reports content size).
- vello GPU tiles behind `brows12-render::compositor` (deterministic CPU
  path remains the fallback).
- Will-change hints → layer promotion.

## Platform

- macOS CI lane; Linux aarch64 CI lane.
- Wayland/X11 + Windows DirectComposition reference shells (the C ABI is
  the contract; shells prove it).
- Profile persistence end-to-end (KV schema is in place: cookies, cache,
  web storage, IndexedDB).

## Security hardening

- Site isolation via process-per-origin using the same C ABI seam.
- WASM: decide between wasmi (in-process, interpreter) and wasmtime (JIT)
  when the event model is complete.
