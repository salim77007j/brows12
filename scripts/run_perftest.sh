#!/bin/bash
# v2.1 perf A/B driver: run brows12-ui on a page for N seconds with frame
# metrics, then quit and summarize the per-stage numbers.
#
# Usage: run_perftest.sh <url> <seconds> <tag> [--software] [--no-pump]
#   tag         label used for /tmp/perf_<tag>.log and /tmp/perf_<tag>.png
#   --software  force the CPU lane (the OLD present path: readback + composite)
#   --no-pump   set BROWS12_NO_PUMP=1 (pre-fix event-loop behavior)
#
# Robustness: the cmd fifo is held open O_RDWR (never blocks), so slow
# software-lane startups (llvmpipe init) cannot deadlock the writer side.
set -u
URL="$1"; SECS="$2"; TAG="$3"; shift 3
SOFTWARE=0; NOPUMP=0
for a in "$@"; do
  case "$a" in
    --software) SOFTWARE=1 ;;
    --no-pump)  NOPUMP=1 ;;
  esac
done

UI=/home/z/my-project/brows12-servo/target/release/brows12-ui
OUTLOG=/tmp/perf_${TAG}.log
OUTPNG=/tmp/perf_${TAG}.png
D=/tmp/prun; rm -rf $D; mkdir -p $D
mkfifo $D/cmd $D/ev 2>/dev/null

pkill -9 -f brows12-ui 2>/dev/null
pkill -9 -f "http.server 8123" 2>/dev/null
sleep 0.5

cd /home/z/my-project/scripts
python3 -m http.server 8123 --bind 127.0.0.1 > /tmp/http.log 2>&1 &
HTTP_PID=$!
sleep 0.5

# Event pump FIRST so the UI's event-fifo write-end never blocks.
cat $D/ev > $D/events.log &
CAT_PID=$!

EXTRA=()
[ "$SOFTWARE" = "1" ] && EXTRA+=(--software)

env DISPLAY=:99 \
    LD_LIBRARY_PATH=$HOME/.local/gl/usr/lib/x86_64-linux-gnu \
    __EGL_VENDOR_LIBRARY_FILENAMES=$HOME/.local/gl/usr/share/glvnd/egl_vendor.d/50_mesa.json \
    XDG_RUNTIME_DIR=/tmp/xdg RUST_LOG=error \
    BROWS12_FRAME_METRICS=1 \
    BROWS12_LOOP_TRACE=1 \
    BROWS12_NO_PUMP=$( [ "$NOPUMP" = "1" ] && echo 1 || echo 0 ) \
    BROWS12_UI_CMD_FIFO=$D/cmd BROWS12_UI_EVENT_FIFO=$D/ev \
    BROWS12_UI_SNAPSHOT=$OUTPNG \
    "$UI" "${EXTRA[@]}" > $OUTLOG 2>&1 &
UI_PID=$!

# Hold the cmd fifo open O_RDWR — open() with O_RDWR returns immediately
# even with no reader; the UI's reader thread picks lines up when ready.
exec 3<>$D/cmd

# Wait for the UI to finish startup (start event or 20 s wall).
for i in $(seq 1 100); do
  kill -0 $UI_PID 2>/dev/null || break
  grep -q "^start pid=" $D/events.log 2>/dev/null && break
  sleep 0.2
done
# Wait for the start page's loaded event before navigating — issuing
# `webview.load()` while the previous load is in-flight races the engine
# (measured: the new load never starts on the slow software lane).
for i in $(seq 1 150); do
  kill -0 $UI_PID 2>/dev/null || break
  grep -q "^loaded tab=" $D/events.log 2>/dev/null && break
  sleep 0.2
done

echo "<OMNI> $URL" >&3
echo "<RETURN>" >&3
sleep "$SECS"                # measurement window
cp $OUTPNG /tmp/prun/snap.png 2>/dev/null
echo "<QUIT>" >&3
sleep 2
kill $UI_PID 2>/dev/null
kill $CAT_PID 2>/dev/null
kill $HTTP_PID 2>/dev/null
exec 3>&-

echo "=== $TAG: perf summary (each line = 60-frame rolling window) ==="
grep "brows12 perf \[frame" $OUTLOG | tail -6
echo "=== $TAG: loop trace (0.5 Hz) ==="
grep "brows12 looptrace" $OUTLOG | tail -4
echo "=== $TAG: events (loaded/frames) ==="
grep -E "loaded|snapshot" $D/events.log | tail -4
echo "=== $TAG: log tail ==="
tail -3 $OUTLOG
ls -la $OUTPNG 2>/dev/null
