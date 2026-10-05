#!/usr/bin/env python3
"""v2.1 Phase 1.3 — memory sampling via the headless (brows-servo) path.

brows-perf's loop stalls on pages whose engine teardown hangs (MDN,
bbc.com/news — pre-existing engine behavior, JSON blocked behind
teardown). `brows-servo` writes its JSON before teardown, so the same
tree-sampling protocol (area3_compare) can measure peak RSS through the
headless binary instead. Reports the embedder completion criterion plus
raw engine flag from the brows-servo JSON.
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
from p11_cnn_diag import Sampler, GL_ENV  # noqa: E402

ROOT = pathlib.Path(__file__).resolve().parent.parent


def measure_headless(url, settle_ms, timeout_ms):
    out = pathlib.Path("/tmp/p13-headless.json")
    if out.exists():
        out.unlink()
    bin_ = ROOT / "target/release/brows-servo"
    cmd = [str(bin_), "--url", url,
           "--settle-ms", str(settle_ms), "--timeout-ms", str(timeout_ms),
           "--json", str(out)]
    proc = subprocess.Popen(cmd, env=GL_ENV,
                            stdout=subprocess.DEVNULL,
                            stderr=subprocess.DEVNULL)
    with Sampler(lambda: [proc.pid]) as s:
        # The process may hang in teardown after writing JSON: exit the
        # sampler as soon as the JSON appears, then kill the tree.
        deadline = (timeout_ms + settle_ms) / 1000 + 90
        start = time.time()
        while time.time() < start + deadline:
            rc = proc.poll()
            if out.exists() or rc is not None:
                break
            time.sleep(0.25)
        if out.exists() and rc is None:
            time.sleep(1.0)  # let the sampler catch the tail
            proc.kill()
    if not out.exists():
        return None
    r = json.loads(out.read_text())
    return {
        "rss_kb": s.peak, "pss_kb": s.pss_at_peak, "uss_kb": s.uss_at_peak,
        "peak_kb": max(s.hwm, s.peak), "observed_peak_kb": s.peak,
        "report": r,
    }


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--url", required=True)
    ap.add_argument("--settle-ms", type=int, default=6000)
    ap.add_argument("--timeout-ms", type=int, default=45000)
    ap.add_argument("--out", required=True)
    args = ap.parse_args()

    r = measure_headless(args.url, args.settle_ms, args.timeout_ms)
    if r is None:
        print("[fail] no JSON from brows-servo")
        sys.exit(1)
    rep = r.pop("report")
    data = {
        "url": args.url,
        "peak_mb": round(r["peak_kb"] / 1024, 1),
        "rss_kb": r["rss_kb"],
        "peak_kb": r["peak_kb"],
        "pss_kb": r["pss_kb"],
        "complete": rep["complete"],
        "complete_criterion": rep["complete_criterion"],
        "all_resources_complete": rep["all_resources_complete"],
        "load_complete_ms": rep["load_complete_ms"],
        "frames": rep["frames"],
        "total_ms": rep["total_ms"],
    }
    pathlib.Path(args.out).write_text(json.dumps(data, indent=1))
    print(json.dumps(data, indent=1))


if __name__ == "__main__":
    main()
