#!/usr/bin/env python3
"""Measure idle CPU% and RSS for a brows12-ui process with N tabs.

Protocol: launches the UI under Xvfb with automation FIFOs, opens N tabs,
navigates each to the target URL, waits for all loads, then samples
/proc/<pid>/stat (utime+stime) and /proc/<pid>/status (VmRSS) for a window.

Usage: measure_idle.py --bin PATH --tabs 10 --url https://example.com --secs 30 --out out.json
"""
import argparse, json, os, re, subprocess, sys, threading, time

XVFB_ENV = {
    **os.environ,
    "DISPLAY": ":99",
    "LD_LIBRARY_PATH": os.path.expanduser("~/.local/gl/usr/lib/x86_64-linux-gnu"),
    "__EGL_VENDOR_LIBRARY_FILENAMES": os.path.expanduser(
        "~/.local/gl/usr/share/glvnd/egl_vendor.d/50_mesa.json"),
    "XDG_RUNTIME_DIR": "/tmp/xdg",
    "RUST_LOG": "error",
}

def read_cpu_ticks(pid):
    with open(f"/proc/{pid}/stat", "rb") as f:
        parts = f.read().split()
    utime, stime = int(parts[13]), int(parts[14])
    hz = os.sysconf("SC_CLK_TCK")
    return (utime + stime) / hz

def read_rss_kb(pid):
    try:
        with open(f"/proc/{pid}/status") as f:
            for line in f:
                if line.startswith("VmRSS:"):
                    return int(line.split()[1])
    except OSError:
        pass
    return None

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--bin", required=True)
    ap.add_argument("--tabs", type=int, default=10)
    ap.add_argument("--url", default="https://example.com/")
    ap.add_argument("--secs", type=float, default=30.0)
    ap.add_argument("--out", default=None)
    ap.add_argument("--extra-env", action="append", default=[])
    args = ap.parse_args()

    env = {**XVFB_ENV}
    for kv in args.extra_env:
        k, _, v = kv.partition("=")
        env[k] = v

    d = "/tmp/b12-measure"
    os.makedirs(d, exist_ok=True)
    for f in os.listdir(d):
        os.unlink(os.path.join(d, f))
    cmd_fifo, evt_fifo = f"{d}/cmd", f"{d}/evt"
    os.mkfifo(cmd_fifo); os.mkfifo(evt_fifo)

    proc = subprocess.Popen(
        [args.bin], env={**env,
                         "BROWS12_UI_CMD_FIFO": cmd_fifo,
                         "BROWS12_UI_EVENT_FIFO": evt_fifo},
        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)

    # Reader thread: open event FIFO (blocks until UI side opens it).
    events = []
    def reader():
        with open(evt_fifo) as f:
            for line in f:
                events.append(line.strip())
                if line.startswith("quit"):
                    return
    t = threading.Thread(target=reader, daemon=True)
    t.start()

    # Open the command FIFO for writing (UI opens its end at startup).
    w = open(cmd_fifo, "w")
    pid = None
    deadline = time.time() + 60
    while pid is None and time.time() < deadline:
        for ev in events:
            if ev.startswith("start "):
                pid = int(re.search(r"pid=(\d+)", ev).group(1))
                break
        time.sleep(0.05)
    if pid is None:
        print(json.dumps({"error": "no start event (timeout)"}))
        proc.kill(); sys.exit(1)

    print(f"[measure] pid={pid}, opening {args.tabs} tabs -> {args.url}", flush=True)
    def cmd(line):
        w.write(line + "\n"); w.flush()

    for i in range(args.tabs):
        if i > 0:
            cmd("<NEWTAB>")
        time.sleep(0.3)
        cmd(f"<OMNI> {args.url}")
        cmd("<RETURN>")
        time.sleep(0.7)

    # Wait until all tabs report loaded (each tab emits one loaded on complete).
    deadline = time.time() + 120
    while time.time() < deadline:
        loaded = sum(1 for e in events if e.startswith("loaded "))
        if loaded >= args.tabs:
            break
        time.sleep(0.25)
    loaded = sum(1 for e in events if e.startswith("loaded "))
    print(f"[measure] loaded events: {loaded}/{args.tabs}", flush=True)
    time.sleep(3.0)  # settle

    rss_kb = read_rss_kb(pid)
    # Idle sampling window.
    c0, t0 = read_cpu_ticks(pid), time.time()
    samples = []
    for _ in range(int(args.secs)):
        time.sleep(1.0)
        samples.append((read_cpu_ticks(pid), read_rss_kb(pid)))
    c1, t1 = read_cpu_ticks(pid), time.time()
    cpu_pct = 100.0 * (c1 - c0) / (t1 - t0)
    rss_end = samples[-1][1]

    out = {
        "binary": args.bin,
        "tabs": args.tabs,
        "url": args.url,
        "loaded_events": loaded,
        "idle_cpu_percent": round(cpu_pct, 3),
        "window_secs": round(t1 - t0, 1),
        "rss_after_load_kb": rss_kb,
        "rss_end_kb": rss_end,
        "rss_per_tab_kb": round(rss_end / max(args.tabs, 1), 1) if rss_end else None,
    }
    print(json.dumps(out, indent=2))
    if args.out:
        with open(args.out, "w") as f:
            json.dump(out, f, indent=2)
    cmd("<QUIT>")
    try:
        proc.wait(timeout=10)
    except subprocess.TimeoutExpired:
        proc.kill()
    w.close()

if __name__ == "__main__":
    main()
