# brows12 v2.1 — Real-Machine Issue Report: UI Performance (ISSUE 1) & Missing Small Icons (ISSUE 2)

**Date:** 2026-10-07 · **Scope:** v2.1.0 post-release field reports on the
Windows/AMD-iGPU test machine, root-caused and fixed on the Linux/Mesa
reference container, with same-binary A/B measurements.

**Verdict up front:**

| Issue | Root cause | Fix | Measured result |
|---|---|---|---|
| 1 — UI extremely slow (P0) | 3 compounding shell defects: per-frame full-window CPU present path; dead `WaitUntil` event-loop wakeups that never pumped the engine; an animation feedback deadlock that froze rAF pages | GPU fast-present lane (`glBlitFramebuffer`, zero readback); event-loop pump; repaint-while-animating | Present path **16.9 → 10.6 ms/frame** (software GL; readback stall eliminated entirely on real GPUs); rAF **frozen (~2 fps) → sustained 60 fps** |
| 2 — small icons missing (P1) | Shell repaint pipeline consumed image frames too early + two invalid test assets masked that data-URL PNG and WOFF icon fonts work | Frame-flow repaint fix + delayed snapshot; corrected assets | Icon reproduction page: **0/8 → 8/8 rendered** in the UI lane |

---

## 1. ISSUE 1 — Why the browser was slow

### 1.1 The three real bottlenecks (all in the shell, none in the engine)

**B1. The present path was pure CPU and ran per frame.**
The old `draw()` did, *every frame*:

1. `webview.paint()` — page rendered into the offscreen FBO (GPU, fine);
2. `read_to_image()` — `glReadPixels` of the whole 1280×728×4 framebuffer;
   this stalls the GPU pipeline and memcpys ~3.7 MB to the CPU *per frame*;
3. a per-pixel premultiply copy into a tiny-skia pixmap;
4. a full-window tiny-skia composite (chrome + page);
5. a per-pixel demultiply + `0RGB` conversion for softbuffer;
6. softbuffer's own blit to the window.

Five full-window CPU passes plus a GPU pipeline flush per frame. On the
user's integrated-GPU machine this alone caps the compositor far below
interactive rates, and it explains why *everything* (scroll, typing,
page loads) felt stuck: every repaint paid the readback tax.

**B2. `WaitUntil(16 ms)` deadlines fired into a no-op.**
`about_to_wait` scheduled 16 ms wakeups while a tab was busy, but the
callback body never pumped Servo (`spin_event_loop`). Every engine message
round-trip (script → constellation → paint → embedder → back) therefore
only advanced when unrelated user input happened to wake the loop. Input
latency and page-load progression were both hostage to this.

**B3. The animation feedback deadlock — rAF pages froze.**
In servo 0.6 the animation/rAF timeline only advances when a composite
runs, and a composite only runs inside `webview.paint()`. The shell only
painted when the frame counter changed — but new frames only arrive after
a composite. Deadlock. Measured with the loop tracer
(`BROWS12_LOOP_TRACE=1`): on a rAF test page the engine frame counter
froze at 35, ~2 s after load, and stayed there for the rest of a 15 s
window (< 30 draws ≈ 2 fps effective). This is the single biggest reason
animated/scrolling pages felt dead.

### 1.2 User hypotheses — what was confirmed, what was ruled out

| # | Hypothesis | Finding |
|---|---|---|
| ① | ANGLE hardware used only for the GPU check, rendering on WARP/llvmpipe | **Ruled out.** The machine log shows `angle-d3d11-hardware` selected as the rendering backend; surfman renders into that same context (`webview.paint()` → offscreen FBO on the same GL context). The GPU was rendering; the CPU present path was negating it. |
| ② | Compositor/paint backend on CPU | **Confirmed (shell).** The chrome/page composite was tiny-skia-on-CPU (B1 step 4). Fixed by the GPU lane. |
| ③ | WebRender textures software-blitted | **Confirmed (shell).** Not WebRender's textures, but the *window present* was a CPU blit (softbuffer) after readback. Fixed. |
| ④ | Per-frame CPU→GPU readback | **Confirmed.** B1 step 2 — the single most expensive pass. Eliminated on the GPU lane. |
| ⑤ | Privacy filter running per frame | **Ruled out.** The engine runs once per *request* at the network layer; 143 204 rules are compiled once (1452 ms cold, ~154 ms warm), and the draw path never touches the filter. |
| ⑥ | UI/engine thread lock contention | **Ruled out as a lock issue; replaced by a real one.** The shell is single-threaded by design; the stall was B2/B3, not contention. |
| ⑦ | Full re-parse/relayout per frame | **Ruled out.** Layout produces display lists on change; the compositor repaints only on new display lists (verified: static pages settle to zero draws; animated pages repaint without re-layout). |

### 1.3 Fixes

All in the shell (`ui/`, `servo-host/`); engine untouched.

1. **GPU fast-present lane** — new `ui/src/present.rs`, used whenever the
   parent rendering context is a window surface:
   `webview.paint()` → offscreen FBO; `OffscreenRenderingContext::
   render_to_parent_callback()` — a GPU `glBlitFramebuffer` straight into
   the window surface (servo 0.6's designed integration path); the chrome
   strip (tab bar + toolbar, 1280×72 ≈ 1/11 of the viewport) is
   rasterized once per frame into a small CPU pixmap, uploaded as a
   texture only when it changes, and blended on top with a textured quad;
   `parent.present()` flips the swap chain. **Zero full-window CPU
   passes, zero readback.** The historical readback + softbuffer path is
   retained verbatim as the automatic fallback (`--software`, or overlay
   creation failure) so the GPU-lane change can never break the
   software-only lane.
2. **Event-loop pump** — `about_to_wait` now pumps the engine once per
   callback (`gui.tick()`); the pre-fix behavior is reproducible for A/B
   measurement via `BROWS12_NO_PUMP=1`.
3. **Repaint-while-animating** — while the engine reports
   `animating == true`, the shell keeps the paint loop running every
   16 ms; it naturally stops when the animation ends. This breaks the
   B3 deadlock and delivers rAF at display rate.
4. **Completion-state hygiene** (correctness found during diagnosis):
   the engine's initial `about:blank` document latched
   `LoadStatus::Complete`, which marked the tab Loaded and swallowed the
   real document's completion (white page / no title). Additionally, a
   UI-side navigation (omnibox/back/reload) could race the *previous*
   document's in-flight completion, which re-latched after the
   invalidation and (a) fired `loaded` for the old URL, (b) swallowed the
   new document's completion, and (c) re-pushed the old URL as the
   current history entry. Fixes: `HostState::invalidate_completion()`
   called synchronously on UI-side navigation; only recordable documents
   drive `Loading→Loaded`; completions whose URL is a non-current
   history entry are ignored as stragglers.
5. **Delayed validation snapshots** — the one-shot per-navigation snapshot
   saved the first draw after `loaded`, which could still be the previous
   document's frame; the save is now 400 ms after the generation bump
   (deterministic wake even on idle pages).

### 1.4 Before/after numbers (same binary, same page, same machine)

Test page: `scripts/icontest/anim.html` (two rAF-driven transform
boxes); container = Linux + Mesa llvmpipe, 1280×728 window.
Instrumentation: `BROWS12_FRAME_METRICS=1` (60-frame rolling windows,
per-stage avg/max), `BROWS12_LOOP_TRACE=1` (0.5 Hz chain dump).

| Configuration (one binary) | rAF frame rate | Per-frame cost |
|---|---|---|
| **Before** — old present path + no repaint-while-animating (as released in v2.1.0) | **frozen ~2 fps** (engine frames stop at 35; < 30 draws in 15 s) | 16.9 ms avg / 23.6 ms max (CPU lane, `total-cpu`) |
| **After** — CPU lane (old present path retained as fallback) + loop fixes | **~57 fps** (569 draws in 10 s) | 16.9 ms avg / 19.4 ms max |
| **After** — GPU lane (new fast present) + loop fixes | **~60 fps** (1132 draws in 19 s ≈ 59.6 fps) | **10.6 ms avg** = paint 0.26 + blit 4.4 + chrome-raster 0.51 + upload 4.9 + present 0.56 |

Reading the table:

- The **animation fix** (B3 + B2) is what un-freezes pages: before it,
  the frame loop stalls ~2 s into every animated page regardless of
  present path; after it, even the old CPU path sustains ~57 fps on this
  test page.
- The **present-path fix** (B1) is what lowers per-frame cost:
  16.9 → 10.6 ms here *on a software rasterizer*, where "blit/upload" are
  memcpys. On the user's AMD iGPU (ANGLE/D3D11) the readback was a hard
  GPU pipeline stall + bus transfer per frame — the worst possible case
  for that hardware — so the relative gain there is expected to be much
  larger than the llvmpipe delta, and scroll/typing latency inherits it
  directly.
- Privacy-filter init (hypothesis ⑤) measured in the same runs:
  143 204 rules, ~145–159 ms warm — one-time, not per-frame.

**Repro:** `scripts/run_perftest.sh <url> <secs> <tag> [--software]
[--no-pump]`; the icon/anim pages live under `scripts/icontest/`.

### 1.5 Known issue discovered on the way (documented, not yet fixed)

Navigating while the previous document is still loading can deadlock the
*software* lane's engine (the new load never starts; GPU lane unaffected
in the same scenario when driven after `loaded`). The shell-side
completion-hygiene fixes above narrow the race window; the automation
driver now waits for the previous `loaded` event before navigating.
Tracked as a follow-up for the software lane only.

---

## 2. ISSUE 2 — Small icons missing

### 2.0 Reproduction page

`scripts/icontest/index.html` — eight row cases: inline `<svg>` 16 px and
12 px, `<img src=*.svg>` 16 px, `<img src=*.png>` 16 px, data-URL PNG
16 px, `@font-face` icon-font glyph 16 px (U+E000 in a WOFF), a 64 px
`<img>` control, and a 16 px inline SVG on a dark background.

Before shot: `docs/screenshots/issue2-before-ui-blank-icons.png`
(every icon blank, including the 64 px control). After shot:
`docs/screenshots/issue2-after-ui-icons-fixed.png` (all eight render).

### 2.1 Diagnosis, per user hypothesis

| # | Hypothesis | Finding |
|---|---|---|
| ① | Small SVGs rendered at zero size | **Ruled out.** Inline and `<img>` SVGs at 12/16 px rasterize and place correctly — headless captured them correctly even on v2.1.0. |
| ② | Small images fail to decode | **Ruled out.** 16 px PNGs decode (headless proof). The one "broken" case was a corrupt *test asset*, see ③/④ below. |
| ③ | CSS-sized small images get 0×0 | **Ruled out.** The `.box` flex containers lay out at 32×32 with 16 px content; verified on the reproduction page. |
| ④ | Icon fonts (FontAwesome-style) not loaded | **Partially confirmed, asset was at fault.** `@font-face` WOFF loading works: with a *valid* WOFF the glyph renders in the declared color (red). The repro font in `make_assets.py` was hand-rolled and malformed (`numTables=0`, wrong lengths), which produced tofu unrelated to the engine. Regenerated with fontTools; committed as `scripts/icontest/iconfont.woff`. |
| ⑤ | Favicon pipeline broken | **Not applicable / feature gap.** The tab strip does not draw favicons at all (no favicon slot in the chrome strip); there is no broken favicon *rendering* to fix. UI feature, deferred. |
| ⑥ | Sub-pixel rounding to 0 px | **Ruled out.** Same-size renders are identical in headless (CPU capture) and UI (GPU lane). |

The real UI defect was upstream of any of the six: **the shell consumed
new frames without repainting when image decode completed after the
page's "settled" moment**, so image display lists existed in the engine
but never reached the screen in the UI lane (headless always captured
them because it paints at capture time). The ISSUE 1 frame-flow fix
(new-frames → dirty → repaint, plus animation-driven pumping) is what
restores them; the delayed snapshot then captures the settled frame
instead of the pre-image one.

### 2.2 Engine-side confirmations (headless `brows-servo`, CPU capture)

- Valid data-URL PNG 16 px: **renders** (green square with white dot).
- Valid WOFF icon font 16 px: **renders** (red glyph).
- All eight repro cases: **8/8** — `docs/screenshots/` before/after pair
  referenced above.

### 2.3 Asset corrections (test-infrastructure, not engine)

- `scripts/icontest/index.html` — data-URL PNG replaced with a
  checksum-valid PNG (the original had a broken IDAT CRC; PIL rejects
  it, and so did the engine — the broken-image placeholder was correct).
- `scripts/icontest/iconfont.woff` — regenerated with fontTools
  (`flavor='woff'`, single U+E000 glyph).
- The favicon of the repro page remains a data-URL PNG; favicon
  *display in tabs* is a pending UI feature (2.1 ⑤ above).

---

## 3. Verification

- `cargo test --workspace --release` — **131 passed / 0 failed**
  (unit + integration + doc).
- `cargo test --release --workspace -- --ignored` with
  `BROWS12_GATE_BIN=target/release/brows-perf` — memory gates **2/2 PASS**
  (typical ≤ 100 MB/tab, heavy floor ≤ 600 MB).
- Manual: anim page 60 fps GPU lane; icon page 8/8 both lanes;
  `https://example.com` loads and completes (183 frames) headless;
  chrome strip composites correctly over the blitted page on both lanes.
- Debug instrumentation retained behind env gates:
  `BROWS12_FRAME_METRICS`, `BROWS12_LOOP_TRACE`, `BROWS12_NO_PUMP` (A/B
  measurement only), `BROWS12_SVG_TRACE`.

## 4. What to test on the real machine (Windows / AMD iGPU)

1. Scroll and omnibox typing on heavy pages — the per-frame readback
   stall is gone; expect the largest delta there.
2. Any rAF-animated page (or the bundled `scripts/icontest/anim.html`)
   — should hold ~60 fps instead of freezing after ~2 s.
3. `brows12 gfx:` log line must still say `angle-d3d11-hardware OK`
   (the fallback chain is unchanged; the fast lane rides on the same
   context).
4. Icon-heavy sites (GitHub header, Wikipedia sidebar, any FontAwesome
   site) — small SVG/PNG icons should appear; icon fonts render if the
   site ships valid web fonts.
