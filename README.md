# Brows12

**The featherweight browser engine.** An independent, privacy-first browser
engine core written in Rust, powered by [QuickJS-ng](https://quickjs-ng.github.io/).

[![CI](https://github.com/salim77007j/brows12/actions/workflows/ci.yml/badge.svg)](https://github.com/salim77007j/brows12/actions/workflows/ci.yml)

Brows12 is a **complete engine backend** — networking, HTML/CSS parsing,
style cascade, layout, rasterization, JavaScript, storage and privacy —
designed so a UI layer can be built on top of a small, clean API
(`brows12-api`, Rust or C ABI). It is not a Chrome fork: every subsystem is
assembled from the fastest actively-maintained Rust crates of 2026, chosen
by benchmark and memory profile (see [LIBRARY_CHOICES.md](LIBRARY_CHOICES.md)).

```
URL ─▶ HTTPS upgrade ─▶ ad/tracker filter ─▶ HTTP/1.1·2·3 (rustls TLS 1.3)
    ─▶ HTML5 parse (html5ever) ─▶ arena DOM
    ─▶ CSS parse (lightningcss) ─▶ selector match ─▶ cascade
    ─▶ layout (taffy + cosmic-text shaping)
    ─▶ display list ─▶ raster (tiny-skia) ─▶ framebuffer ─▶ your UI
    ─▶ scripts (QuickJS-ng realm: fetch, XHR, WebSocket, Workers, DOM, storage)
```

## Highlights

- **Memory-safe by construction.** The whole engine is safe Rust; the only C
  in the process is QuickJS-ng itself, sandboxed behind `rquickjs` with hard
  heap and stack limits plus an interrupt hook.
- **Lean by design.** Arena DOM (no `Rc` cycles), display-list rendering,
  aggressive tab suspension (DOM/style/layout trees dropped on idle), and a
  content-addressed disk cache. Idle tabs cost a URL string, a title and a
  snapshot.
- **Private by default.** Network-layer ad/tracker blocking (Brave's
  `adblock` engine, uBlock-compatible filter syntax), HTTPS upgrade, CHIPS
  cookie partitioning, CNAME-cloaking detection via DoH CNAME chains, canvas/
  navigator/WebGL anti-fingerprinting hooks, and **zero telemetry**.
- **Modern protocols.** HTTP/1.1 + HTTP/2 + TLS 1.3 (rustls, ring, Mozilla
  roots), RFC 8484 DNS-over-HTTPS with a hand-rolled, fuzzed wire-format
  implementation, and HTTP/3 over QUIC (`quinn` + `h3`) behind a feature flag.
- **Measure everything.** Criterion micro-benchmarks per subsystem, an engine
  compare tool, and browser-comparison scripts. CI gates on `clippy -D
  warnings` and full test passes on Linux **and** Windows.

## Quick start (Rust)

```toml
[dependencies]
brows12-api = { path = "path/to/brows12/api" }
```

```rust
use brows12_api::prelude::*;

let browser = Browser::builder()
    .viewport(1280, 800)
    .privacy(|p| {
        p.block_ads = true;
        p.https_upgrade = true;
    })
    .build();

let events = browser.subscribe();          // EngineEvent stream for the UI
let tab = browser.new_tab();
tab.load_url("https://example.com")?;

if let Some(frame) = tab.frame() {
    // premultiplied RGBA8, upload straight into your compositor
    let (w, h) = (frame.width, frame.height);
    let _rgba = frame.rgba_premultiplied();
}
```

## Quick start (C ABI)

```c
#include "brows12.h"

B12Engine *e = b12_engine_new("{\"viewport\":[1280,800]}");
B12Tab *t = b12_tab_new(e);
b12_tab_load(t, "https://example.com");

uint32_t w, h; const uint8_t *data; size_t len;
if (b12_tab_frame(t, &w, &h, &data, &len) == 0) {
    /* blit `data` (w*h*4 bytes) */
}
```

Build the static lib: `cargo build -p brows12-api --release --features capi`.

## Repository layout

| Path | Crate | What lives here |
|---|---|---|
| `engine/` | `brows12-engine` | Orchestrator: tabs, navigation pipeline, suspension, event bus |
| `js/` | `brows12-js` | QuickJS-ng runtime, event loop, DOM/fetch/XHR/WebSocket/Worker/storage bindings |
| `networking/` | `brows12-net` | hyper + rustls HTTP client, DoH, HTTP/3 (feature) |
| `parsing/html` | `brows12-html` | Arena DOM + html5ever HTML5 parsing |
| `parsing/css` | `brows12-css` | lightningcss parsing, selector matcher, cascade |
| `layout/` | `brows12-layout` | taffy box tree + cosmic-text measurement |
| `rendering/` | `brows12-render` | Display lists, tiny-skia rasterizer, compositor |
| `storage/` | `brows12-storage` | Cookies + CHIPS, web storage, HTTP cache, IndexedDB |
| `privacy/` | `brows12-privacy` | Adblock engine, anti-fingerprinting, HSTS, policies |
| `api/` | `brows12-api` | UI-facing facade + C ABI (`api/include/brows12.h`) |
| `tests/` | `brows12-tests` | Cross-crate integration suite |
| `benchmarks/` | `brows12-benchmarks` | Criterion benches + browser compare tooling |
| `fuzz/` | `brows12-fuzz` | cargo-fuzz targets (parsers + wire formats) |
| `ci/` | — | Local CI runner; workflows in `.github/workflows/` |
| `docs/` | — | Architecture, API reference, performance, privacy, roadmap |

## Performance

Measured by `brows12-compare` (debug build on the development machine;
release numbers land in CI artifacts):

| metric | Brows12 |
|---|---|
| cold start (engine construct) | 23 ms |
| tab creation | < 1 ms |
| internal page first paint | 11 ms |
| local page, full pipeline (fetch→parse→style→layout→render) | 6 ms |

Micro-benchmarks (HTML parse throughput, cascade, layout, raster, JS realm)
run under Criterion: `cargo bench -p brows12-benchmarks`. Cross-browser
methodology and scripts: [docs/PERFORMANCE.md](docs/PERFORMANCE.md).

## Scope & honesty

Brows12 v0.1 is a real, working engine core: it loads pages over the
network, applies CSS, runs page JavaScript against a live DOM, renders
frames, blocks trackers and keeps cookies partitioned. It is **not** yet a
drop-in Chromium replacement — inline flow (mixed inline boxes on one line),
floats, tables, media queries and form controls are the known gaps, tracked
in [docs/ROADMAP.md](docs/ROADMAP.md). We would rather ship a fast,
transparent core with an honest gap list than a slow one with a long feature
table.

## Building

```sh
cargo build --workspace
cargo test --workspace
cargo bench -p brows12-benchmarks --no-run
```

See [docs/BUILDING.md](docs/BUILDING.md) for platform notes (Linux, macOS,
Windows) and feature flags.

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE).
