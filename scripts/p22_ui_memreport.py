#!/usr/bin/env python3
"""v2.1 Phase 2 — product (brows12-ui) multi-tab idle RSS + engine memory report.

Launches brows12-ui under Xvfb with automation FIFOs, opens N tabs of one
URL, waits for loads, samples RSS/CPU for an idle window, sends
<MEMREPORT> and captures the memreport event (engine explicit breakdown),
then quits. Usage:

  p22_ui_memreport.py --tag base [--tabs 10] [--url https://example.com]
                      [--idle-secs 20] [--extra-env K=V ...]
"""
import argparse
import json
import os
import pathlib
import re
import subprocess
import sys
import threading
import time

ROOT = pathlib.Path(__file__).resolve().parent.parent
OUT = ROOT / "docs/perf-artifacts/v21"

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
    return (utime + stime) / os.sysconf("SC_CLK_TCK")


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
    ap.add_argument("--tag", required=True)
    ap.add_argument("--tabs", type=int, default=10)
    ap.add_argument("--url", default="https://example.com")
    ap.add_argument("--idle-secs", type=float, default=20.0)
    ap.add_argument("--extra-env", action="append", default=[])
    a = ap.parse_args()

    env = {**XVFB_ENV}
    for kv in a.extra_env:
        k, _, v = kv.partition("=")
        env[k] = v

    d = "/tmp/b12-p22"
    os.makedirs(d, exist_ok=True)
    for f in os.listdir(d):
        os.unlink(os.path.join(d, f))
    cmd_fifo, evt_fifo = f"{d}/cmd", f"{d}/evt"
    os.mkfifo(cmd_fifo)
    os.mkfifo(evt_fifo)

    bin_ = ROOT / "target/release/brows12-ui"
    err_f = open(f"{d}/stderr.log", "wb")
    proc = subprocess.Popen(
        [str(bin_)], env={**env, "BROWS12_UI_CMD_FIFO": cmd_fifo,
                          "BROWS12_UI_EVENT_FIFO": evt_fifo},
        stdout=subprocess.DEVNULL, stderr=err_f)

    events = []
    def reader():
        with open(evt_fifo) as f:
            for line in f:
                events.append(line.strip())
                if line.startswith("quit"):
                    return
    t = threading.Thread(target=reader, daemon=True)
    t.start()

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
        print(json.dumps({"error": "no start event"}))
        proc.kill()
        sys.exit(1)

    def cmd(line):
        w.write(line + "\n")
        w.flush()

    for i in range(a.tabs):
        if i > 0:
            cmd("<NEWTAB>")
        time.sleep(0.3)
        cmd(f"<OMNI> {a.url}")
        cmd("<RETURN>")
        time.sleep(0.7)

    deadline = time.time() + 120
    while time.time() < deadline:
        if sum(1 for e in events if e.startswith("loaded ")) >= a.tabs:
            break
        time.sleep(0.25)
    loaded = sum(1 for e in events if e.startswith("loaded "))
    time.sleep(3.0)

    rss_after_load = read_rss_kb(pid)
    c0, t0 = read_cpu_ticks(pid), time.time()
    samples = []
    for _ in range(int(a.idle_secs)):
        time.sleep(1.0)
        samples.append((read_cpu_ticks(pid), read_rss_kb(pid)))
    c1, t1 = read_cpu_ticks(pid), time.time()
    cpu_pct = 100.0 * (c1 - c0) / (t1 - t0)
    rss_end = samples[-1][1]

    # Engine memory report via the automation FIFO.
    cmd("<MEMREPORT>")
    memreport = None
    deadline = time.time() + 30
    while time.time() < deadline:
        for e in events:
            if e.startswith("memreport "):
                memreport = e
                break
        if memreport:
            break
        time.sleep(0.25)

    # smaps rollup: where does the non-instrumented RSS actually live?
    smaps = None
    if pid:
        try:
            regions = []
            with open(f"/proc/{pid}/smaps") as f:
                cur = None
                for line in f:
                    if re.match(r"^[0-9a-f]+-[0-9a-f]+ ", line):
                        parts = line.split()
                        name = parts[5] if len(parts) > 5 else "[anon]"
                        cur = {"name": name, "rss": 0}
                    elif cur is not None and line.startswith("Rss:"):
                        cur["rss"] = int(line.split()[1])
                        regions.append(cur)
                        cur = None
            agg = {}
            for r in regions:
                key = ("[heap/anon]" if r["name"] in ("[anon]", "[heap]", "[stack]")
                       else r["name"])
                agg[key] = agg.get(key, 0) + r["rss"]
            top = sorted(agg.items(), key=lambda kv: -kv[1])[:14]
            smaps = [{"mapping": k, "rss_mb": round(v / 1024, 1)} for k, v in top]
        except OSError:
            pass

    out = {
        "tag": a.tag,
        "binary": str(bin_),
        "tabs": a.tabs,
        "url": a.url,
        "loaded_events": loaded,
        "idle_cpu_percent": round(cpu_pct, 3),
        "window_secs": round(t1 - t0, 1),
        "rss_after_load_kb": rss_after_load,
        "rss_end_kb": rss_end,
        "rss_per_tab_kb": round(rss_end / max(a.tabs, 1), 1) if rss_end else None,
        "memreport": memreport,
        "smaps_top": smaps,
        "events_tail": events[-25:],
        "stderr_tail": open(f"{d}/stderr.log", errors="replace").read()[-2000:],
    }
    OUT.mkdir(parents=True, exist_ok=True)
    (OUT / f"p2_ui_{a.tag}.json").write_text(json.dumps(out, indent=2))
    print(json.dumps({k: v for k, v in out.items() if k not in ("memreport", "events_tail", "stderr_tail")}, indent=2))
    if smaps:
        print("smaps top:")
        for s in smaps:
            print(f"  {s['rss_mb']:8.1f} MB  {s['mapping']}")
    if memreport:
        rep = memreport.split("engine_report=", 1)[-1]
        try:
            r = json.loads(rep)
            print("engine explicit total MB:", round(r["explicit_total_bytes"] / 1024 / 1024, 1))
            for p, v in r["top_paths"][:12]:
                print(f"  {v/1024/1024:8.1f} MB  {p}")
        except Exception as ex:
            print("memreport parse:", ex, memreport[:400])
    cmd("<QUIT>")
    try:
        proc.wait(timeout=10)
    except subprocess.TimeoutExpired:
        proc.kill()
    w.close()


if __name__ == "__main__":
    main()
