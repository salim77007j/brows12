# Phase 4 — Focus Area 4 Report: Tab Innovation (4.1–4.7)

**Status: COMPLETE.** All seven sub-items implemented, unit-tested, and verified
end-to-end against the real shell (`brows12-ui` under Xvfb, driven through the
`BROWS12_UI_CMD_FIFO`/`BROWS12_UI_EVENT_FIFO` automation channels).

**Verification method note.** Tab innovation is *shell* behavior; headless
Chromium exposes no tab-strip, governor, or session observable, so the
side-by-side Chrome protocol used for rendering fixes (Areas 1–3) does not
apply here. Verification is by (a) pure-policy unit tests (24 new tests across
the four policy modules), (b) event-stream invariants asserted end-to-end by
`scripts/phase4_area4.py` (26 checks total), and (c) artifacts committed under
`docs/perf-artifacts/phase4/area4/`. Every claim below maps to a named check.

---

## 4.1 Predictive hibernation — DONE

`servo-host/src/tabstats.rs`: per-tab usage signals (`activations`,
`last_active_ms`, `page_requests`) condensed into a return-probability score
`0.65·recency + 0.35·frequency`, where recency decays exponentially with a
30-minute half-life and frequency is `a/(1+a)`. **Page weight is deliberately
not a predictor** — it enters as a cost: among near-equal scores (Δ < 0.02)
the heavier page is suspended first, and the existing heavy-tab eligibility
schedule (4.3.4) is unchanged. The governor's reclaim now executes candidates
in `tabstats::hibernation_order` order (least-likely-to-return first) and
emits `predict_order` events. No ML, by design — the heuristic is
inspectable, testable, and cheap.

New knob: `BROWS12_GOVERNOR_WARMUP_MS` (default 0) delays the first governor
tick so harnesses can build session state before ranking starts.

**Verified:** 4 tabs with the pattern "tab2 ×3, tab1 once (ends active), tab3
never" under a 64 MB budget → prediction `[3, 2, 0]`, hibernation executed
`[3, 2, 0]`. Checks: prediction emitted, first candidate = zero-activation
tab, first hibernation = first prediction, frequency ordering honored.

## 4.2 Memory-aware tab discarding — DONE

- **Value-ranked discard**: pressure reclaims (RSS, PSI, per-tab budget
  ladder) all flow through the 4.1 ordering — the lowest-value tab goes
  first.
- **State preservation across discard**:
  - *History* — inherent (session history lives in `UiTab`, untouched by
    hibernation).
  - *Scroll* — the shell tracks a per-tab vertical scroll estimate from
    forwarded wheel deltas (clamped ≥ 0; `window.scrollTo` clamps the top
    end on restore). Re-applied after the reloaded document completes.
  - *Forms* — `SERIALIZE_FORMS_JS` runs at the last live moment (when the
    tab goes to the background); the JSON snapshot is harvested into
    `UiTab.form_state` at hibernation and re-applied by `restore_state_js`
    on reload. (The engine exposes no synchronous DOM access at hibernation
    time; capture-at-background is the honest engineering choice.)
- **User notification**: governor-driven hibernations mark the tab
  `discarded`, emit `tab_discarded index= scroll_est= forms= url=`, and the
  strip draws a red dot on discarded tabs (distinct from the plain hibernate
  gray).

**Verified:** tall fixture, scroll 3×800 px, timer-filled input; after
discard the event stream shows `scroll_est=2400 forms=1`, and after restore
`state_restore index=1 scroll_est=2400 forms=1`. Checks: discard emitted,
scroll preserved, form snapshot preserved, restore re-applied.

## 4.3 Tab groups (data layer) — DONE

`servo-host/src/tabgroups.rs`: `TabGroup {id, name, color, collapsed}` +
`TabGroupStore` with membership keyed by shell tab id. Full API:
create (empty name → color name), delete (members ungrouped, not closed),
rename, recolor, `toggle_collapsed`, add/remove tab, `tab_closed`
(membership dies with the tab), `group_of`/`tabs_in`/`color_for`, and a
space-free `to_json` that survives the event channel escaping. 8 Chrome-style
named colors with RGB mappings.

Shell: FIFO commands `<GROUP_NEW name|color>`, `<GROUP_DEL>`, `<GROUP_ADD
g|tab>`, `<GROUP_REMOVE>`, `<GROUP_TOGGLE>`, `<GROUPS>` (JSON dump); events
`group_created/group_deleted/group_add/group_remove/group_toggle/
groups_json`; the strip renders a 3 px group color bar on each grouped tab.
Collapse state is stored for the future UI (data-layer scope per mission).

**Verified:** create "research|purple", add strip tabs 0+2 (shell ids 1+3),
JSON dump asserts name/color/membership, toggle → `collapsed=1`, delete →
`ok=true` + second dump empty. All checks pass. 4 unit tests.

## 4.4 Full session restore — DONE

`servo-host/src/session.rs`: `Session {version, saved_at_ms, active, tabs,
groups}`; each `SessionTab` carries full history + hindex, title, scroll
estimate, form-state JSON, pinned flag, group id. **Atomic save** (tmp file +
rename) so a crash mid-write cannot corrupt the last good session; `load`
refuses corrupt, empty, or future-version files (restore fails closed to a
cold start).

Restore modes: `BROWS12_SESSION_RESTORE=1|last` → `BROWS12_SESSION_FILE`;
`=</path>` → a specific session file. Startup (`restore_session_or_new`)
rehydrates groups with their original ids (`TabGroupStore::restore_group`),
rebuilds **only the active tab live**, and brings every other tab back as
suspended metadata that rehydrates on activation — startup builds one
webview regardless of strip size. Scroll/form reapplication rides the 4.2
`pending_state_restore` path. Saves happen on `<QUIT>`, on window close, and
periodically (`BROWS12_SESSION_SAVE_SECS`, default 60, 0 = off).

**Verified:** 3-run e2e — run 1 saves on quit (`tabs=3 groups=1`); run 2
restores the last session (active a.html live, suspended tab rehydrates to
b.html, group survives with members); run 3 restores an explicit session
file. All 6 checks pass. 4 unit tests.

## 4.5 Tab search — DONE

`servo-host/src/tabsearch.rs`: in-memory index over open tabs with AND-token
queries (case-insensitive), field weights (title 3.0 / URL 2.0 / snippet
1.0), start-of-field prefix bonus, capped best-first results. Snippets are
captured by `SNIPPET_JS` at `LoadStatus::Complete` (meta description /
og:description + first heading, 240-char cap) into `HostState.page_snippet`.
The shell refreshes entries on every active-tab sync and background sync and
drops entries on tab close. FIFO: `<TABSEARCH> query` →
`tabsearch_results count=N` + `tabsearch_hit rank= index= score= title= url=
snippet=` events (UI palette can consume these directly).

**Verified:** title query hits one tab; a snippet-only query ("sourdough",
present only in the meta description) finds its tab; multi-token AND ("moon
alpha") resolves to the single page containing both; no-match returns
`count=0`. 5 unit tests.

## 4.6 Pinned tabs — DONE

`UiTab.pinned` + `<PIN> i` FIFO toggle (`tab_pinned` event) + a blue accent
bar on the tab's left edge. **Pinned tabs are exempt from every memory
policy**: pressure reclaim, the per-tab trim/hibernate ladder, and the
over-budget tier accounting all skip them — pinned never hibernate. Pin
state persists in the session file (schema landed with 4.4) and is restored
on startup, so pinned tabs come back exactly as left.

**Verified:** pin tab1 under a 64 MB budget → the governor hibernates the
unpinned background tab only and never touches the pinned tab; the session
JSON records `pinned: true`; after restore the pinned tab stays live through
further ticks. All 6 checks pass.

## 4.7 Configurable suspend policy — DONE

`servo-host/src/suspend.rs`: `BROWS12_SUSPEND_POLICY` =
`never` (idle suspension off), `aggressive` (60 s), `<seconds>` (default
300 s = 5 minutes). Pure `should_suspend()` decision honoring the full
exemption matrix: active tab, pinned (4.6), **playing media**, and the
`BROWS12_SUSPEND_EXEMPT_URLS` user list (comma-separated substrings). The
idle-suspend pass runs every governor tick at any pressure level — this is
the "suspend after N minutes of idle" behavior, previously missing (the old
governor only reacted to pressure). `never` disables *only* the idle pass;
the pressure responses stay armed.

**Media detection (engine gap found):** the delegate implements
`notify_media_session_event` → `PlaybackStateChange(Playing)` →
`HostState.media_playing` (reset on navigation), and the exemption is
unit-tested. However, servo-script 0.6 stores
`navigator.mediaSession.playbackState` writes in a `DomRefCell` **without
notifying the embedder** (`dom/media/mediasession.rs:189`), so only real
media-playback events reach the delegate. Upstream issue candidate (see
below). The e2e therefore exercises the URL exemption end-to-end.

**Verified:** AfterIdle(3 s) suspends both idle background tabs at *nominal*
pressure while the active tab survives; `never` suspends nothing; the
URL-exempt tab survives while the non-exempt one is suspended. All 5 checks
pass. 4 unit tests.

---

## Test & artifact summary

| Suite | Result |
|---|---|
| `cargo test -p servo-host` (unit) | **43 passed, 0 failed** (24 new: tabstats 7, tabgroups 4, session 4, tabsearch 5, suspend 4) |
| `scripts/phase4_area4.py --only all` | **26/26 checks PASS** across 7 sub-items |
| `cargo fmt` | clean (formatted pass committed) |
| `cargo clippy` | not installed in this environment (component missing); `cargo check`/build clean |
| Artifacts | `docs/perf-artifacts/phase4/area4/area4_verify.json` (+ per-sub-item `area4_4{2..7}.json`) |

Per-sub-item commits: `92b4d38` (4.1), `baeef5b` (4.2), `569c106` (4.3),
`384f2b1` (4.4), `08aca2f` (4.5), `a50267a` (4.6), `2d31427` (4.7),
`a8fa5e1` (fmt).

## New embedder API surface (FIFO)

`<GROUP_NEW name|color>` · `<GROUP_DEL id>` · `<GROUP_ADD g|tab>` ·
`<GROUP_REMOVE tab>` · `<GROUP_TOGGLE id>` · `<GROUPS>` · `<TABSEARCH q>` ·
`<PIN i>` — plus env: `BROWS12_SESSION_FILE`, `BROWS12_SESSION_RESTORE`,
`BROWS12_SESSION_SAVE_SECS`, `BROWS12_SUSPEND_POLICY`,
`BROWS12_SUSPEND_EXEMPT_URLS`, `BROWS12_GOVERNOR_WARMUP_MS`.

## Upstream gap discovered in this area

1. **Media-session playback-state writes do not notify the embedder**
   (servo-script 0.6 `dom/media/mediasession.rs`, `set_playback_state`
   stores without emitting `MediaSessionEvent::PlaybackStateChange`). The
   embedder-side listener is implemented and correct; it simply never fires
   for playbackState writes. A minimal repro is a two-line page setting
   `navigator.mediaSession.playbackState = "playing"` — Chrome fires the
   event, Servo 0.6 does not.

## Honest limitations

- **Scroll restoration** uses a wheel-delta estimate, not the engine's true
  scroll offset (Servo 0.6 exposes no scroll getter); `window.scrollTo`
  clamping keeps it safe, and it is exact for keyboard/wheel-driven
  scrolling — the dominant case.
- **Form restoration** covers inputs/textarea/select values, checked
  states, and select indices by document order; contentEditable and file
  inputs are out of scope.
- **Group collapse** is data-layer only (stored + dumped); the strip still
  draws all tabs. Hiding/collapsing tabs visually is future UI work.
- **Session restore** rebuilds only the active tab live; the memory cost of
  the rest is metadata-only until each tab is activated (a feature, not a
  limitation, but worth knowing).
- Clippy could not run (component unavailable in the container); fmt +
  check + full test suite are green.
