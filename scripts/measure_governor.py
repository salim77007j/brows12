#!/usr/bin/env python3
"""Measure the memory governor reclaiming RSS over time: load N tabs,
then let the governor hibernate background tabs and watch VmRSS drop.

Usage: measure_governor.py --bin PATH --tabs 10 --url https://example.com/ --out out.json
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
    # Governor tuned for the measurement: hibernate after 8s inactive.
    "BROWS12_TAB_SUSPEND_SECS": "8",
    "BROWS12_GOVERNOR_INTERVAL_MS": "1000",
    "BROWS12_MEM_BUDGET_MB": os.environ.get("B12_GOV_BUDGET_MB", "384"),
}

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
    ap.add_argument("--watch-secs", type=float, default=75.0)
    ap.add_argument("--out", default="/tmp/b12-governor.json")
    args = ap.parse_args()

    d = "/tmp/b12-gov"
    os.makedirs(d, exist_ok=True)
    for f in os.listdir(d):
        os.unlink(os.path.join(d, f))
    cmd_fifo, evt_fifo = f"{d}/cmd", f"{d}/evt"
    os.mkfifo(cmd_fifo); os.mkfifo(evt_fifo)

    proc = subprocess.Popen(
        [args.bin], env={**XVFB_ENV,
                         "BROWS12_UI_CMD_FIFO": cmd_fifo,
                         "BROWS12_UI_EVENT_FIFO": evt_fifo},
        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)

    events = []
    def reader():
        with open(evt_fifo) as f:
            for line in f:
                events.append(line.strip())
                if line.startswith("quit"):
                    return
    threading.Thread(target=reader, daemon=True).start()

    w = open(cmd_fifo, "w")
    def cmd(line):
        w.write(line + "\n"); w.flush()

    def wait_for(pred, timeout=120, what="event"):
        end = time.time() + timeout
        while time.time() < end:
            if any(pred(e) for e in events):
                return next(e for e in events if pred(e))
            time.sleep(0.1)
        raise TimeoutError(what)

    wait_for(lambda e: e.startswith("start "), 60, "start")
    pid = int(re.search(r"pid=(\d+)", next(e for e in events if e.startswith("start"))).group(1))
    print(f"[gov] pid={pid}, opening {args.tabs} tabs", flush=True)

    for i in range(args.tabs):
        if i > 0:
            cmd("<NEWTAB>")
        time.sleep(0.3)
        cmd(f"<OMNI> {args.url}")
        cmd("<RETURN>")
        time.sleep(0.7)
    loaded = sum(1 for e in events if e.startswith("loaded "))
    end = time.time() + 120
    while time.time() < end:
        loaded = sum(1 for e in events if e.startswith("loaded "))
        if loaded >= 2 * args.tabs:   # each tab emits loaded twice (premature + real)
            break
        time.sleep(0.25)
    print(f"[gov] loaded events: {loaded}", flush=True)
    time.sleep(2.0)

    rss_before = read_rss_kb(pid)
    print(f"[gov] RSS before governor window: {rss_before/1024:.1f} MB", flush=True)

    # Watch RSS as the governor hibernates eligible background tabs.
    series = [(0.0, read_rss_kb(pid))]
    t0 = time.time()
    while time.time() - t0 < args.watch_secs:
        time.sleep(2.5)
        series.append((round(time.time() - t0, 1), read_rss_kb(pid)))
        print(f"[gov] t={series[-1][0]:5.1f}s rss={series[-1][1]/1024:7.1f} MB", flush=True)

    gov_events = [e for e in events if e.startswith("governor ") or e.startswith("hibernate ")]
    rss_after = read_rss_kb(pid)

    cmd("<QUIT>")
    time.sleep(2.0)
    try:
        proc.wait(timeout=10)
    except subprocess.TimeoutExpired:
        proc.kill()
    w.close()

    out = {
        "tabs": args.tabs,
        "url": args.url,
        "budget_mb": XVFB_ENV["BROWS12_MEM_BUDGET_MB"],
        "suspend_after_secs": 8,
        "rss_before_kb": rss_before,
        "rss_after_kb": rss_after,
        "rss_freed_kb": rss_before - rss_after,
        "series": series,
        "governor_events": gov_events,
    }
    with open(args.out, "w") as f:
        json.dump(out, f, indent=1)
    print(json.dumps({k: out[k] for k in
                      ["rss_before_kb", "rss_after_kb", "rss_freed_kb"]}, indent=1))
    print(f"[gov] {len(gov_events)} governor/hibernate events; report in {args.out}")

if __name__ == "__main__":
    main()
