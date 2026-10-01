# API Reference (UI layer)

The UI layer integrates through `brows12-api`. Two surfaces: a Rust facade
and a C ABI. Event flow is push (subscribe) + pull (frame handles).

## Rust facade

```rust
use brows12_api::prelude::*;

let browser = Browser::builder()
    .viewport(1280, 800)                       // CSS pixels
    .user_agent("MyShell/1.0 (Brows12)")       // identity
    .max_live_pages(6)                          // LRU suspension budget
    .privacy(|p| p
        .block_ads(true)                        // network filter list
        .https_upgrade(true)                    // rewrite http→https
        .strict_fingerprinting(false)           // hardened navigator spoofs
        .block_cname_cloaking(true)             // DoH CNAME guard
        .block_third_party_cookies(false))      // CHIPS instead of blocking
    .build();

let mut events = browser.subscribe();           // tokio broadcast channel
let tab = browser.new_tab();                    // Arc<Tab>
```

### Tab operations

| Method | Returns | Notes |
|---|---|---|
| `tab.load_url(&str)` | `Result<(), EngineError>` | Full pipeline: fetch → parse → style → layout → paint → scripts → repaint |
| `tab.load_url_from_string(&str html, &str virtual_url)` | `Result<(), EngineError>` | Internal pages, zero network |
| `tab.frame()` | `Option<Frame>` | Newest framebuffer (generation-counted) |
| `tab.title()` / `tab.url()` | `String` | Current document metadata |
| `tab.is_live()` | `bool` | false while suspended |
| `tab.suspend()` | `()` | Drop DOM/style/layout/JS; keep snapshot |
| `tab.resume()` | `Result<(), EngineError>` | Warm-cache reload |

### Frame

```rust
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub generation: u64,     // monotonically increasing per tab
    // pixmap backing store:
    //   rgba_premultiplied() -> &[u8]  (w*h*4, row-major, top-left origin)
    //   encode_png() -> Vec<u8>
}
```

The UI owns presentation: upload `rgba_premultiplied()` to a texture (the
recommended path — it is exactly what a GPU uploader expects) or blit it.
`generation` lets the shell skip stale frames: keep painting the newest
generation you have seen.

### Events

```rust
match events.blocking_recv()? {
    EngineEvent::NavigationStarted { tab, url } => { /* show spinner */ }
    EngineEvent::NavigationCommitted { tab, url } => { /* url bar update */ }
    EngineEvent::FrameReady { tab, generation } => { /* request repaint */ }
    EngineEvent::LoadFinished { tab, title } => { /* stop spinner */ }
    EngineEvent::RequestBlocked { tab, url, reason } => { /* shield counter */ }
    EngineEvent::Console { tab, message } => { /* devtools console */ }
    EngineEvent::Suspended { tab } | EngineEvent::Resumed { tab } => {}
}
```

The channel is a tokio broadcast: multiple subscribers (compositor, UI,
history recorder) each get every event.

## C ABI (feature `capi`)

`cargo build -p brows12-api --release --features capi` produces
`libbrows12_api.a` plus `api/include/brows12.h`. Configuration flows in as
JSON (`{"viewport":[w,h],"user_agent":"...","max_live_pages":n}`) so the ABI
is additive-stable. Handles are opaque; frames are borrowed pointers valid
until the tab's next load. See the header for signatures.

## What the UI layer is expected to provide

- Windowing/input, tabs UI, omnibox, downloads — the engine has no opinion.
- Frame scheduling: paint on `FrameReady`, composite `Tab::frame()` of the
  newest generation.
- History/session bookkeeping on top of `NavigationCommitted` events.
- Settings persistence; engine config is plain data (`EngineConfig`).
