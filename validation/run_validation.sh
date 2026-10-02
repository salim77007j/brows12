#!/usr/bin/env bash
# run_validation.sh — end-to-end validation of brows12-ui under Xvfb.
#
# Drives the real browser window through the automation channels:
#   - commands in:  BROWS12_UI_CMD_FIFO  (<OMNI> <RETURN> <BACK> <FORWARD>
#                   <RELOAD> <NEWTAB> <SWITCH> n <SLEEP> ms <QUIT>)
#   - events out:   BROWS12_UI_EVENT_FIFO (tab_created/nav/loaded/load_error/
#                   omni/back/forward/reload/newtab/switch/quit lines)
# Every navigation is WAITED FOR via its real event before the screenshot,
# so captures never race the page loads.
#
# Usage: validation/run_validation.sh
# Exit:  0 = all steps passed; 1 = a wait/shot/assert failed.
set -u

REPO="$(cd "$(dirname "$0")/.." && pwd)"
BIN="$REPO/target/release/brows12-ui"
XDRV="$REPO/validation/x11driver/x11driver"
RUN="$REPO/validation/run"
SHOTS="$REPO/screenshots"
DISP="${BROWS12_VALIDATE_DISPLAY:-:77}"
JOURNAL="$RUN/events.log"

mkdir -p "$RUN" "$SHOTS"
: > "$JOURNAL"
FAILED=0
BROWSER_PID=""

cleanup() {
  [ -n "$BROWSER_PID" ] && kill "$BROWSER_PID" 2>/dev/null
  [ -n "${XVFB_PID:-}" ] && kill "$XVFB_PID" 2>/dev/null
}
trap cleanup EXIT

die() { echo "FAIL: $*" | tee -a "$JOURNAL" >&2; FAILED=1; exit 1; }

# ---- preflight ----------------------------------------------------------
[ -x "$BIN" ] || die "missing $BIN (build with: cargo build --release -p brows12-ui)"
[ -x "$XDRV" ] || die "missing $XDRV (build: cargo build --release --manifest-path validation/x11driver/Cargo.toml)"

# ---- Xvfb ---------------------------------------------------------------
Xvfb "$DISP" -screen 0 1280x800x24 -nolisten tcp &
XVFB_PID=$!
SOCKET="/tmp/.X11-unix/X${DISP#:}"
for i in $(seq 1 50); do [ -S "$SOCKET" ] && break; sleep 0.1; done
[ -S "$SOCKET" ] || die "Xvfb $DISP did not come up"

# ---- launch browser -----------------------------------------------------
# Some environments (no-root sandboxes) lack libxkbcommon-x11; point
# XKB_LIB_DIR at a directory containing the extracted .so files.
if [ -n "${XKB_LIB_DIR:-}" ]; then
  export LD_LIBRARY_PATH="$XKB_LIB_DIR${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
fi
CMD_FIFO="$RUN/cmd.fifo";  rm -f "$CMD_FIFO";  mkfifo "$CMD_FIFO"
EVT_FIFO="$RUN/events.fifo"; rm -f "$EVT_FIFO"; mkfifo "$EVT_FIFO"

DISPLAY="$DISP" BROWS12_UI_CMD_FIFO="$CMD_FIFO" BROWS12_UI_EVENT_FIFO="$EVT_FIFO" \
  "$BIN" > "$RUN/ui-stdout.log" 2> "$RUN/ui-stderr.log" &
BROWSER_PID=$!

# fd8 = events (rdwr open never blocks on a fifo and keeps a writer end
# alive so the UI's open succeeds immediately); fd9 = commands.
# (High fds: wrappers around this script use 3/4 for their own plumbing.)
exec 8<>"$EVT_FIFO"
exec 9>"$CMD_FIFO"

# ---- helpers ------------------------------------------------------------
LAST_LINE=""

wait_evt() { # $1 = ERE the line must match, $2 = timeout seconds
  local pat="$1"
  local tmo="${2:-90}"
  local deadline=$(( $(date +%s) + tmo ))
  while [ "$(date +%s)" -lt "$deadline" ]; do
    if IFS= read -r -t 1 line <&8; then
      echo "$line" >> "$JOURNAL"; echo "EVT> $line"
      if [[ "$line" =~ $pat ]]; then LAST_LINE="$line"; return 0; fi
    elif kill -0 "$BROWSER_PID" 2>/dev/null; then
      :
    else
      die "browser exited while waiting for /$pat/ (see $RUN/ui-stderr.log)"
    fi
  done
  die "timeout ${tmo}s waiting for event /$pat/"
}

send() { echo "$1" >&9; }

shot() { # $1 = screenshot basename (no ext)
  sleep 0.4   # let the final paint land after the awaited event
  DISPLAY="$DISP" "$XDRV" shot "$SHOTS/$1.png" >/dev/null \
    || die "screenshot $1 failed"
  echo "SHOT> $1.png"
}

# ---- 1. start page ------------------------------------------------------
wait_evt '^tab_created id=[0-9]+' 30
wait_evt '^loaded tab=[0-9]+ .*title=New_Tab' 30
shot 01_newtab_start

# ---- 2. typed search query ---------------------------------------------
send '<OMNI> rust programming language'
wait_evt '^omni text=rust_programming_language$' 15
send '<SLEEP> 700'                       # caret blink + paint
shot 02_typed_query

# ---- 3. search results (omnibox -> default search engine) ---------------
send '<RETURN>'
wait_evt '^return input=rust_programming_language$' 15
wait_evt '^loaded .*url=.*bing\.com/search' 120
TITLE="${LAST_LINE##*title=}"; TITLE="${TITLE//_/ }"
[[ "$TITLE" == *rust* ]] || die "search results title unexpected: $TITLE"
shot 03_search_results

# ---- 4. example.com -----------------------------------------------------
send '<OMNI> example.com'
send '<RETURN>'
wait_evt '^loaded .*url=https://example\.com' 90
shot 04_example

# ---- 5. wikipedia -------------------------------------------------------
send '<OMNI> en.wikipedia.org/wiki/Rust_(programming_language)'
send '<RETURN>'
wait_evt '^loaded .*url=.*wikipedia\.org.*Rust' 120
shot 05_wikipedia

# ---- 6. github ----------------------------------------------------------
send '<OMNI> github.com'
send '<RETURN>'
wait_evt '^loaded .*url=.*github\.com' 120
shot 06_github

# ---- 7. rust-lang.org ---------------------------------------------------
send '<OMNI> www.rust-lang.org'
send '<RETURN>'
wait_evt '^loaded .*url=.*rust-lang\.org' 120
shot 07_rustlang

# ---- 8. reload (used for real; no dedicated screenshot slot) ------------
send '<RELOAD>'
wait_evt '^reload url=' 15
wait_evt '^loaded .*url=.*rust-lang\.org' 120
echo "OK> reload completed"

# ---- 9. second tab, different site --------------------------------------
send '<NEWTAB>'
wait_evt '^tab_created id=' 30
wait_evt '^loaded tab=[0-9]+ .*title=New_Tab' 30
send '<OMNI> example.org'
send '<RETURN>'
wait_evt '^loaded .*url=https://example\.org' 90
shot 08_two_tabs

# ---- 10. switch back to tab 0 and go back/forward ------------------------
send '<SWITCH> 0'
wait_evt '^switch index=0' 15
send '<BACK>'
wait_evt '^back from=' 15
wait_evt '^loaded .*url=.*github\.com' 120   # history: index 5 = rust-lang -> back = index 4 = github
shot 09_back
send '<FORWARD>'
wait_evt '^forward from=' 15
wait_evt '^loaded .*url=.*rust-lang\.org' 120
shot 10_forward

# ---- shutdown -----------------------------------------------------------
send '<QUIT>'
wait_evt '^quit$' 15
wait "$BROWSER_PID" 2>/dev/null || true
echo "OK> browser exited cleanly"

# ---- screenshot sanity: no two captures may be identical -----------------
if md5sum "$SHOTS"/[0-9]*.png | awk '{print $1}' | sort | uniq -d | grep -q .; then
  md5sum "$SHOTS"/[0-9]*.png
  die "duplicate screenshots detected (window did not visibly change)"
fi

echo
echo "PASS: validation complete — journal: $JOURNAL"
exit 0
