# integration — engine ↔ shell contract

How `ui/` (the browser shell) talks to the engine, end to end. This is the
surface any other front end (egui/Slint/GPU shell) would reuse unchanged.

## Process model

```
┌─────────────────────────── UI thread (winit event loop) ───────────────────────────┐
│  chrome.rs   tab strip + toolbar + omnibox drawing & hit testing (tiny-skia)       │
│  model.rs    UiTab state, input normalization (URL vs search), event/channel types │
│  main.rs     winit ApplicationHandler: input → UiCmd, UiMsg → repaint              │
└───────────────▲──────────────────────────────────────────────────┬────────────────┘
        UiMsg (mpsc)                                  UiCmd (mpsc)
┌───────────────┴──────────────────────────────────────────────────▼────────────────┐
│                     engine_host.rs — owns Browser + all Tabs                      │
│  Tabs are !Send: every engine call happens here. 30 ms poll loop services         │
│  UiCmd requests, drains the engine broadcast channel, and publishes the           │
│  newest Frame of each tab back to the UI thread.                                  │
└────────────────────────────────────────────────────────────────────────────────────┘
                                   brows12-api (facade)
```

The shell never touches engine internals: it only sees `Frame`
(ARGB snapshot for blitting) and `EngineEvent` (navigation/status stream).

## Commands (UI → host): `model::UiCmd`

| Command | Engine call |
|---|---|
| `NewTab` | `Browser::new_tab()` + `Tab::load_url_from_string(START_HTML, "brows12://start")` |
| `CloseTab { id }` | drop the `Arc<Tab>` + `engine().enforce_memory_budget()` |
| `Navigate { tab, url }` | `Tab::load_url(url)` (start URL special-cased to the internal page) |
| `Scroll { tab, dy }` | `Tab::set_scroll(Tab::scroll_y() + dy)` |

## Events (host/engine → UI): `model::UiMsg`

| Message | Source | Shell reaction |
|---|---|---|
| `TabCreated { id }` | host | bind pending tab slot, seed `brows12://start` history |
| `Frame { tab, frame }` | `EngineEvent::FrameReady` → `Tab::frame()` | cache snapshot; repaint if active |
| `Engine(NavigationCommitted)` | engine | sync history entry with committed URL |
| `Engine(LoadFinished)` | engine | tab title + `Loaded` status |
| `LoadResult { tab, Err }` | `Tab::load_url` | tab `Error(e)` status (red in toolbar) |
| `Inject(cmd)` | automation FIFO | routed through the same handlers as real input |

## Automation channels (validation hook)

* `BROWS12_UI_CMD_FIFO` — newline commands: `<OMNI> …`, `<TEXT> …`,
  `<RETURN>`, `<BACK>`, `<FORWARD>`, `<RELOAD>`, `<NEWTAB>`, `<SWITCH> n`,
  `<SCROLL> dy`, `<SLEEP> ms`, `<QUIT>`.
* `BROWS12_UI_EVENT_FIFO` — newline events: `start`, `tab_created`,
  `nav tab=… url=…`, `loaded tab=… url=… title=…`, `load_error`, `omni`,
  `back`, `forward`, `reload`, `newtab`, `switch index=… url=…`, `quit`.

Both are inert unless the env vars are set; a production run of the binary
has zero automation surface. `validation/run_validation.sh` uses them to
drive the real window and wait for real engine state (no sleeps).

## Privacy defaults baked into the shell

`block_ads = true`, `https_upgrade = true`, `block_cname_cloaking = true`,
`max_live_pages = 8` (see `engine_host::run`). Every load — page,
subresource, fetch, worker — passes the same blocker at the network layer.
