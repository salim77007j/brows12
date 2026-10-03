#!/usr/bin/env python3
"""Diagnose the Phase 3.9 heavy-page scroll stall: which Servo thread
burns the wall clock while wheel-scrolling? Runs brows-perf with a long
scroll window, samples /proc/<pid>/task/*/stat per thread, and prints
CPU seconds per thread name.

Usage: python3 scripts/diag_scroll_threads.py [url] [out.json]
"""
import http.server
import json
import os
import pathlib
import socketserver
import subprocess
import sys
import threading
import time
import functools

ROOT = pathlib.Path(__file__).resolve().parent.parent
SERVO_ENV = {
    **os.environ,
    "DISPLAY": ":99",
    "LD_LIBRARY_PATH": os.path.expanduser("~/.local/gl/usr/lib/x86_64-linux-gnu"),
    "__EGL_VENDOR_LIBRARY_FILENAMES": os.path.expanduser(
        "~/.local/gl/usr/share/glvnd/egl_vendor.d/50_mesa.json"),
    "XDG_RUNTIME_DIR": "/tmp/xdg",
    "RUST_LOG": "error",
}

URL = sys.argv[1] if len(sys.argv) > 1 else "http://127.0.0.1:8126/index.html"
OUT = pathlib.Path(sys.argv[2] if len(sys.argv) > 2
                   else "docs/perf-artifacts/phase3/heavy_scroll_threads.json")
PORT = 8126


def thread_stats(pid):
    out = {}
    base = f"/proc/{pid}/task"
    try:
        tids = os.listdir(base)
    except OSError:
        return out
    for tid in tids:
        try:
            with open(f"{base}/{tid}/stat") as f:
                parts = f.read().rsplit(") ", 1)[1].split()
            utime, stime = int(parts[11]), int(parts[12])
            with open(f"{base}/{tid}/comm") as f:
                name = f.read().strip()
            out[tid] = (name, utime + stime)
        except (OSError, IndexError, ValueError):
            continue
    return out


def main():
    handler = functools.partial(http.server.SimpleHTTPRequestHandler,
                                directory="/tmp/heavy_site")
    socketserver.TCPServer.allow_reuse_address = True
    srv = socketserver.TCPServer(("127.0.0.1", PORT), handler)
    threading.Thread(target=srv.serve_forever, daemon=True).start()

    bin_ = str(ROOT / "target" / "debug" / "brows-perf")
    proc = subprocess.Popen(
        [bin_, "--url", URL, "--settle-ms", "15000",
         "--scroll-secs", "12", "--json", "/tmp/heavy_scroll.json"],
        env=SERVO_ENV, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)

    time.sleep(8)          # engine boot + page load + settle
    t0 = thread_stats(proc.pid)
    t0w = time.monotonic()
    time.sleep(14)         # scroll window (settle 15s ends, 12s scroll)
    t1 = thread_stats(proc.pid)
    t1w = time.monotonic()
    wall = t1w - t0w
    srv.shutdown()

    deltas = []
    for tid, (name, total) in t1.items():
        old = t0.get(tid, (name, total))[1]
        d = (total - old) / os.sysconf("SC_CLK_TCK")
        if d > 0.05:
            deltas.append((d, name, tid))
    deltas.sort(reverse=True)

    report = {
        "url": URL, "wall_secs": round(wall, 2),
        "threads": [{"name": n, "tid": t, "cpu_secs": round(d, 2),
                     "pct": round(100 * d / wall, 1)} for d, n, t in deltas],
        "total_cpu_secs": round(sum(d for d, _, _ in deltas), 2),
    }
    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text(json.dumps(report, indent=2))
    print(json.dumps(report, indent=2))
    proc.wait(timeout=30)


if __name__ == "__main__":
    main()
