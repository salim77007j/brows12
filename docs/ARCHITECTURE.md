# Architecture

## Component map

```text
                    ┌─────────────────────────────────────────────┐
                    │                brows12-api                  │
                    │   Rust facade  │  C ABI (brows12.h)         │
                    └───────────────┬─────────────────────────────┘
                                    │ EngineEvent stream + Tab handles
┌───────────────────────────────────┼──────────────────────────────────────────┐
│                       brows12-engine (orchestrator)                           │
│  ┌──────────┐  ┌──────────┐  ┌───────────┐  ┌──────────┐  ┌───────────────┐  │
│  │  tabs    │  │ pipeline │  │ suspension│  │ budgeter │  │  event bus    │  │
│  └────┬─────┘  └────┬─────┘  └───────────┘  └──────────┘  └───────────────┘  │
│       │             │                                                          │
│  ┌────▼──────────────────────────────┐   ┌────────────────────────────────┐  │
│  │ load_url(url)                     │   │ subsystems (shared, Arc)       │  │
│  │  1. https upgrade        privacy  │   │  net: hyper+rustls (h1/h2/h3)  │  │
│  │  2. blocker check        privacy  │   │  cookies: RFC6265bis + CHIPS   │  │
│  │  3. fetch (+cache rescue) net     │   │  blocker: adblock engine       │  │
│  │  4. parse                html     │   │  storage: redb/memory KV       │  │
│  │  5. stylesheets          css      │   │  cache: content-addressed      │  │
│  │  6. cascade              css      │   │  fonts: cosmic-text FontSystem │  │
│  │  7. layout               layout   │   └────────────────────────────────┘  │
│  │  8. paint gen N          render   │                                       │
│  │  9. scripts              js       │                                       │
│  │ 10. re-style/re-paint    css/js   │                                       │
│  └───────────────────────────────────┘                                       │
└───────────────────────────────────────────────────────────────────────────────┘
```

## Threading model

- **UI thread** — owns `Engine`/`Tab` handles. Tab calls are synchronous
  (blocking) by design: a load runs the whole pipeline and returns. Async
  work lives inside the engine's tokio runtime.
- **Engine runtime** — a 2-worker tokio runtime drives HTTP connections
  (h1/h2/h3), fetches stylesheets/scripts/images concurrently, and runs the
  DoH client.
- **JS realm thread** — `JsRuntime` is `!Send` on purpose: each realm runs
  to completion on the thread that executes page scripts. Its event loop
  interleaves microtasks, OS timers and fetch completions from a channel;
  completed network work is handed across the boundary as plain data.

## The load pipeline (engine/src/tab.rs)

1. **HTTPS upgrade** — `http://` rewritten to `https://` unless the host is
   loopback. HSTS entries force the upgrade regardless of config.
2. **Privacy check** — the document URL goes through the same adblock engine
   as every subresource. Blocked documents render the built-in shield page
   (`brows12://blocked/...`) — no network request is ever made.
3. **Fetch** — hyper client, rustls TLS 1.3, Mozilla roots, redirect loop
   (max 10, POST→GET on 303/302), cookies attached by the storage layer's
   `CookieStore` adapter, CHIPS partitioning applied. Failures fall back to
   the content-addressed HTTP cache.
4. **HTML parse** — html5ever's spec-compliant tree builder, transplanted
   into the arena DOM.
5. **Stylesheets** — inline `<style>` (document order), then external
   `<link rel=stylesheet>` (capped at 12), parsed by lightningcss into owned
   rule storage. UA defaults come from the embedded `ua.css`.
6. **Cascade** — per element: collect matching rules (origin → specificity →
   source order), apply declarations, apply `style=""`, inherit from parent.
7. **Layout** — taffy tree built from the DOM (display:none pruned), text
   leaves measured by cosmic-text, geometry computed for the viewport.
8. **Paint** — display list in tree order; tiny-skia rasterizes backgrounds,
   borders, glyphs (swash alpha masks tinted by computed color, color glyphs
   for emoji) and images into a premultiplied-RGBA framebuffer.
9. **Scripts** — one QuickJS-ng realm per page: glue (classes, registries)
   then each `<script>` in document order (externals fetched concurrently-
   capped). The realm's event loop settles timers and pending fetches.
10. **Re-render** — any script DOM mutation flips a flag on the `DomHandle`;
    the engine re-runs cascade → layout → paint (generation bump) and emits
    `FrameReady`.

## Memory strategy

- **Arena DOM** — nodes live in one `Vec`; dropping a page is O(1).
- **Suspension** — `Tab::suspend()` keeps `{url, title, last frame}` and
  drops the DOM, style map, layout tree and JS realm. Resuming re-loads from
  the warm cache. The engine sweeps live pages LRU-oldest-first whenever
  `max_live_pages` is exceeded.
- **Bounded work** — external stylesheets (12), images (24) and scripts (16)
  per page are capped; the JS realm has hard heap/stack limits; response
  bodies over 1 MiB skip the memory cache layer (disk layer still holds
  them).
- **Cooperative GC** — QuickJS's incremental collector runs inside the
  realm; the engine triggers a full collection only when the realm is idle.

## Failure containment

- Page JS cannot crash the engine: exceptions are captured, reported as
  `Console` events, and the pipeline continues.
- Malformed CSS never aborts a load (lightningcss recovers per-rule).
- The DNS wire parser and cookie/Set-Cookie paths are fuzz targets; the
  HTML/CSS parsers are spec-recovery engines.

## Extension seams for the UI layer

- `Engine::subscribe()` — broadcast `EngineEvent` stream
  (`NavigationStarted`, `NavigationCommitted`, `FrameReady`, `LoadFinished`,
  `RequestBlocked`, `Console`, `Suspended`, `Resumed`).
- `Tab::frame()` — newest premultiplied-RGBA framebuffer with a generation
  counter (upload to a GPU texture or blit directly).
- `Tab::load_url_from_string` — internal pages (`brows12://` scheme) with
  zero network.
- C ABI (`api/include/brows12.h`) for non-Rust shells.
