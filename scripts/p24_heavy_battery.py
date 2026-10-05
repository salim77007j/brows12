#!/usr/bin/env python3
"""v2.1 Phase 2 — heavy-page battery on the new build (reps + summary).

Runs the headless tree-sampler (p13 protocol) over one URL N times and
prints peak-RSS reps. Usage: p24_heavy_battery.py --url U [--reps 3]
[--tag name] [--settle-ms 1500] [--timeout-ms 60000]
"""
import argparse
import json
import pathlib
import sys

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
from p13_headless_mem import measure_headless  # noqa: E402

ROOT = pathlib.Path(__file__).resolve().parent.parent
OUT = ROOT / "docs/perf-artifacts/v21"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--url", required=True)
    ap.add_argument("--tag", required=True)
    ap.add_argument("--reps", type=int, default=3)
    ap.add_argument("--settle-ms", type=int, default=1500)
    ap.add_argument("--timeout-ms", type=int, default=60000)
    a = ap.parse_args()
    results = []
    for i in range(a.reps):
        r = measure_headless(a.url, a.settle_ms, a.timeout_ms)
        r["rep"] = i + 1
        r["peak_rss_mb"] = round(r.get("peak_kb", 0) / 1024, 1)
        results.append(r)
        print(f"rep {i+1}: peak_rss_mb={r.get('peak_rss_mb')} "
              f"complete={r.get('complete')} load_ms={r.get('load_complete_ms')}",
              flush=True)
    peaks = [r["peak_rss_mb"] for r in results if r.get("peak_rss_mb")]
    summary = {
        "tag": a.tag, "url": a.url, "reps": results,
        "peak_rss_mb_reps": peaks,
        "peak_rss_mb_median": sorted(peaks)[len(peaks) // 2] if peaks else None,
    }
    OUT.mkdir(parents=True, exist_ok=True)
    (OUT / f"p2_heavy_{a.tag}.json").write_text(json.dumps(summary, indent=2))
    print(json.dumps({k: summary[k] for k in ("tag", "url", "peak_rss_mb_reps",
                                              "peak_rss_mb_median")}, indent=2))


if __name__ == "__main__":
    main()
