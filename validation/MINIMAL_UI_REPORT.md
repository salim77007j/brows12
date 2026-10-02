# MINIMAL_UI_REPORT — brows12 shell, end-to-end validation

Date: 2026-10-02 · Platform: Linux x86_64 (Xvfb virtual display) · Engine: v0.2.1
Verdict up front: **Yes — the engine works end-to-end through a real browser
window.** Every step below was executed for real; the evidence (screenshots +
raw event journal) is committed in this repository.

## 1. UI stack choice and rationale

**winit 0.30 + softbuffer 0.4 + tiny-skia 0.12**, with the engine's own
`brows12-render` rasterizer (cosmic-text shaping) drawing the chrome text.
Rationale: the engine already ships tiny-skia and cosmic-text, so this stack
adds only two small dependencies (winit for the native window/event loop,
softbuffer for a software-present surface), keeps the whole browser on the
same CPU raster pipeline as the engine (no GPU/wgpu requirement, works
everywhere including Xvfb), and avoids dragging a full UI framework (egui /
Slint / Iced) — and its rendering model — into a shell whose job is to blit
engine frames and draw a thin chrome around them. The entire shell is ~1,300
lines across four files (`ui/src/{main,chrome,model,engine_host}.rs`).

## 2. Exact build & run commands

```bash
# build (release, from repo root)
cargo build --release -p brows12-ui          # → target/release/brows12-ui

# run as a normal desktop app (needs a display; Wayland/X11)
./target/release/brows12-ui

# run the automated end-to-end validation (what produced the screenshots)
cargo build --release --manifest-path validation/x11driver/Cargo.toml
XKB_LIB_DIR=/path/to/xkb-libs bash validation/run_validation.sh
```

Notes:
- On a desktop with `libxkbcommon-x11` installed no env vars are needed.
  In a no-root sandbox, extract `libxkbcommon0`, `libxkbcommon-x11-0`,
  `libxcb1`, `libxcb-xkb1`, `libxcursor1`, `libxrandr2`, `libxi6` .deb
  contents into one directory and point `XKB_LIB_DIR` at it (winit dlopens
  these at startup).
- Automation protocol: the app reads commands from `BROWS12_UI_CMD_FIFO`
  (`<OMNI> text`, `<RETURN>`, `<BACK>`, `<FORWARD>`, `<RELOAD>`, `<NEWTAB>`,
  `<SWITCH> n`, `<SCROLL> dy`, `<SLEEP> ms`, `<QUIT>`) and emits observable
  events on `BROWS12_UI_EVENT_FIFO` (`start`, `tab_created`, `nav`,
  `loaded tab=… url=… title=…`, `load_error`, `omni`, `back`, `forward`,
  `reload`, `newtab`, `switch`, `quit`). Injected input goes through exactly
  the same handlers as real mouse/keyboard input. Both channels are inert
  unless the env vars are set.

## 3. Per-site test results (honest)

All states below were driven through the real window (omnibox typing +
Return, toolbar back/forward/reload, tab strip) via the FIFO automation.
Every navigation was awaited via its real engine event, never by sleep.

| # | State / site | Rendered? | Screenshot | Visible issues |
|---|--------------|-----------|------------|----------------|
| 1 | New-tab start page (`brows12://start`) | **Yes** | `01_newtab_start.png` | none (logo, tagline, omnibox shows `brows12://start`, status `Loaded`) |
| 2 | Omnibox editing (typed query + caret) | **Yes** | `02_typed_query.png` | none — caret blinks at 530 ms, text `rust programming language` visible in omnibox |
| 3 | Search: `rust programming language` → Bing | **Partially** | `03_search_results.png` | Real Bing results ("About 166,000 results", rust-lang.org first hit); dark theme background, some overlapping text top-left, settings toggles drawn as plain rounded rects (partial CSS) |
| 4 | example.com | **Yes** | `04_example.png` | none — white canvas, dark text, correct omnibox URL |
| 5 | en.wikipedia.org/wiki/Rust_(programming_language) | **Partially** | `05_wikipedia.png` | Real content incl. the decoded Rust gear image, sidebar, Tools panel; some overlapping text layers in the header area |
| 6 | github.com | **Partially** | `06_github.png` | Real content (Sign in, Copilot/AI-code sections, developer workflows, customer names) but large overlapping headline text and vertical text columns — layout subset visibly strained |
| 7 | www.rust-lang.org | **Partially** | `07_rustlang.png` | Real content on white (nav links, 12 language entries); hero imagery/branding area mostly absent |
| 8 | Two tabs (tab 2 → example.org) | **Yes** | `08_two_tabs.png` | Tab strip shows both tabs with correct active/close buttons; tab 2 renders example.org text on white |
| 9 | Back (tab 0: rust-lang → github) | **Yes** | `09_back.png` | Correct previous page + omnibox URL after one Back |
| 10 | Forward (tab 0: github → rust-lang) | **Yes** | `10_forward.png` | Correct forward return |

Supporting evidence in this directory:
- `events-20261002.log` — the complete raw event journal of the run
  (every `nav`/`loaded`/`omni`/`back`/`forward`/`reload`/`switch`/`quit`).
- `run/ui-stderr.log` — **empty**: zero panics/warnings from the engine or
  shell during the whole session.
- `run_validation.sh` — the driver; it hard-fails on duplicate screenshots
  (md5) and on any wait timeout, so the PASS is meaningful.

Search engine note: the omnibox routes non-URL queries to Bing. DuckDuckGo
and Google are bot-walled for this engine's request signature (documented in
`docs/CAPABILITY_REPORT.md` §2); Bing serves real results.

## 4. Engine bugs found during UI integration (and their fixes)

1. **Canvas background propagation missing** (CSS 2.1 §14.2). Pages whose
   content is shorter than the viewport (or with no declared background)
   left the rest of the canvas unpainted; in the window this showed as the
   shell's dark backdrop bleeding through under/around page content
   (visible in the first-round captures of example.com/org).
   **Fix**: `rendering/src/display_list.rs` now propagates the root
   element's background to the canvas, falls back to the body's, and
   otherwise paints the initial canvas (white); the fill is pushed first
   and is viewport-anchored (does not scroll). All workspace tests still
   pass; screenshots 04/07/08 show the corrected rendering.
2. **Fresh tabs had no URL in the shell's session history** (engine → UI
   contract): the host navigates a new tab to `brows12://start` itself, so
   the shell's omnibox/back-forward state was empty at birth.
   **Fix**: `ui/src/main.rs` seeds the pending tab slot with `START_URL`
   when `TabCreated` arrives; `NavigationCommitted` keeps the entry in sync
   with the engine's committed URL (HTTPS upgrades included).

No engine crashes, hangs, or memory anomalies were observed during any run.

## 5. What works / what does not

**Works (proven by the run above):** window + viewport blit of engine
frames; omnibox (URL entry, host completion via `https://` prefixing, search
routing, caret editing); Back / Forward / Reload against engine navigation;
multi-tab create/switch/close; status feedback (`Loading…`/`Loaded`/`Error`)
from engine events; per-tab frame snapshots; HTTPS upgrade + ad-block
privacy pipeline active on every load (engine config in `engine_host.rs`);
clean automation channels for scripted driving.

**Does not work yet (honest list):**
- Full CSS layout on heavy sites — github.com/bing show overlapping text and
  missing layout features (see capability report's selector/layout subset).
- Scrollbars: pages scroll via wheel (`<SCROLL>`, MouseWheel →
  `Tab::set_scroll`) but no scrollbar is drawn in the viewport.
- Page zoom, downloads, find-in-page, devtools, history/bookmarks UI,
  window resizing (window is intentionally fixed 1280×800 in this round).
- Canvas-background propagation covers solid colors; `background-image` on
  the root/body does not propagate to the canvas.

## 6. Next UI iteration suggestions

1. Draw a viewport scrollbar synced to `Tab::scroll_y()` / content bounds.
2. Text selection + copy from the page; clipboard paste into the omnibox.
3. Omnibox dropdown (top suggestions from typed prefix + search history).
4. Favicon + spinner in the tab strip driven by existing engine events.
5. Resizable window with DPI-aware chrome (currently fixed-size by design).
6. GPU present path via the existing compositor seam (wgpu surface swap)
   once sites demand faster blits; the shell already speaks in frames, so
   this is an engine-side swap behind `Tab::composite_frame()`.
