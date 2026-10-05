#!/usr/bin/env python3
"""v2.1 Phase 2 — multi-tab idle RSS A/B battery via brows-perf.

Runs brows-perf with N tabs of one URL under the standard GL env, records
per-tab rss_after, total RSS after idle, engine memory report, and the
process VmHWM. Optionally injects extra env (e.g. _RJEM_MALLOC_CONF) for
the allocator A/B. Usage:

  p21_idle_ab.py --tag A_stock [--tabs 10] [--url https://example.com/]
                 [--extra-env K=V ...] [--idle-secs 8] [--engine-report]
"""
import argparse
import json
import os
import pathlib
import subprocess
import sys
import threading
import time

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
from p11_cnn_diag import GL_ENV  # noqa: E402

ROOT = pathlib.Path(__file__).resolve().parent.parent
OUT = ROOT / "docs/perf-artifacts/v21"


def vm(pid, field):
    try:
        with open(f"/proc/{pid}/status") as f:
            for line in f:
                if line.startswith(field):
                    return int(line.split()[1])
    except OSError:
        pass
    return None


def run(tag, tabs, url, idle_secs, engine_report, extra_env):
    bin_ = ROOT / "target/release/brows-perf"
    out = OUT / f"p2_idle_{tag}.json"
    cmd = [str(bin_)]
    for _ in range(tabs):
        cmd += ["--url", url]
    cmd += ["--idle-secs", str(idle_secs), "--json", str(out)]
    if engine_report:
        cmd.append("--engine-report")
    env = {**GL_ENV, **{k: v for k, v in extra_env}}

    proc = subprocess.Popen(cmd, env=env, stdout=subprocess.DEVNULL,
                            stderr=subprocess.DEVNULL)
    # Sample the process RSS while it idles: catch the end-of-idle value.
    samples = []
    stop = threading.Event()
    def sampler():
        while not stop.is_set():
            rss = vm(proc.pid, "VmRSS:")
            hwm = vm(proc.pid, "VmHWM:")
            if rss:
                samples.append((time.time(), rss, hwm))
            time.sleep(0.5)
    t = threading.Thread(target=sampler, daemon=True)
    t.start()
    deadline = idle_secs * tabs + 300
    start = time.time()
    while time.time() < start + deadline:
        if out.exists() and proc.poll() is not None:
            break
        if proc.poll() is not None and not out.exists():
            time.sleep(2)
            if out.exists():
                break
        time.sleep(0.5)
    stop.set()
    time.sleep(0.3)
    alive = proc.poll() is None
    if alive:
        proc.kill()

    result = {"tag": tag, "tabs": tabs, "url": url, "extra_env": extra_env,
              "binary": str(bin_), "process_exited": not alive}
    if out.exists():
        data = json.load(open(out))
        result["perf"] = data
    if samples:
        result["rss_end_kb"] = samples[-1][1]
        result["hwm_peak_kb"] = max((s[2] or 0) for s in samples)
        # average of last 4 samples = settled idle RSS
        tail = [s[1] for s in samples[-4:]]
        result["rss_idle_avg_kb"] = round(sum(tail) / len(tail))
    out.unlink(missing_ok=True)
    (OUT / f"p2_idle_{tag}.summary.json").write_text(json.dumps(result, indent=2))
    return result


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--tag", required=True)
    ap.add_argument("--tabs", type=int, default=10)
    ap.add_argument("--url", default="https://example.com/")
    ap.add_argument("--idle-secs", type=int, default=8)
    ap.add_argument("--engine-report", action="store_true")
    ap.add_argument("--extra-env", action="append", default=[])
    a = ap.parse_args()
    extra = [kv.partition("=")[::2] for kv in a.extra_env]
    r = run(a.tag, a.tabs, a.url, a.idle_secs, a.engine_report, extra)
    tabs = r.get("perf", {}).get("tabs", [])
    per_tab = [t["rss_after_kb"] for t in tabs]
    print(json.dumps({
        "tag": r["tag"],
        "n_tabs": len(tabs),
        "all_complete": all(t["complete"] for t in tabs) if tabs else None,
        "rss_after_per_tab_mb": [round(k / 1024, 1) for k in per_tab],
        "rss_end_mb": round(r["rss_end_kb"] / 1024, 1) if r.get("rss_end_kb") else None,
        "rss_idle_avg_mb": round(r["rss_idle_avg_kb"] / 1024, 1) if r.get("rss_idle_avg_kb") else None,
        "hwm_peak_mb": round(r["hwm_peak_kb"] / 1024, 1) if r.get("hwm_peak_kb") else None,
        "per_tab_avg_mb": round(sum(per_tab) / len(per_tab) / 1024, 1) if per_tab else None,
    }, indent=2))


if __name__ == "__main__":
    main()
