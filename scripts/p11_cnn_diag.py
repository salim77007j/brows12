#!/usr/bin/env python3
"""v2.1 Phase 1.1 — CNN regression staged attribution.

Runs brows-perf on cnn.com N times with fresh engines, denying different
request kinds via BROWS12_DIAG_DENY (servo-host/src/diag.rs):

  stage        env                     what it removes
  full         (unset)                 nothing — the baseline
  no-images    images                  image fetch + decode + texture upload
  no-scripts   scripts                 JS parse/execute + heap growth
  no-both      images,scripts          both

Peak-RSS deltas vs `full` attribute the 481 MB (v2.0.0 baseline, ratio
0.74x vs Chrome) to subsystems. Output JSON mirrors the area3_compare
schema (rss/pss/uss/peak KB per stage) plus per-stage deltas.

Usage:
  python3 scripts/p11_cnn_diag.py --out docs/perf-artifacts/v21/p11_cnn_diag.json
  python3 scripts/p11_cnn_diag.py --stage no-images ...   (single stage)
"""
import argparse
import json
import os
import pathlib
import subprocess
import sys
import threading
import time

ROOT = pathlib.Path(__file__).resolve().parent.parent
URL = "https://www.cnn.com/"

STAGES = [
    ("full", None),
    ("no-images", "images"),
    ("no-scripts", "scripts"),
    ("no-stylesheets", "stylesheets"),
    ("no-frames", "frames"),
    ("no-both", "images,scripts"),
]

GL_ENV = {
    **os.environ,
    "DISPLAY": os.environ.get("BROWS12_X11_DISPLAY", ":99"),
    "LD_LIBRARY_PATH": os.path.expanduser(
        "~/.local/gl/usr/lib/x86_64-linux-gnu"),
    "__EGL_VENDOR_LIBRARY_FILENAMES": os.path.expanduser(
        "~/.local/gl/usr/share/glvnd/egl_vendor.d/50_mesa.json"),
    "LIBGL_ALWAYS_SOFTWARE": "1",
    "XDG_RUNTIME_DIR": "/tmp/xdg",
    "RUST_LOG": "error",
}


# ---- /proc sampling (same protocol as area3_compare.py) -----------------

def proc_tree(pids):
    out = list(pids)
    i = 0
    while i < len(out):
        pid = out[i]
        i += 1
        try:
            for t in os.listdir(f"/proc/{pid}/task"):
                with open(f"/proc/{pid}/task/{t}/children") as f:
                    out.extend(int(c) for c in f.read().split())
        except OSError:
            pass
    return set(out)


def read_kb(pid, path_tpl, field):
    try:
        with open(path_tpl.format(pid=pid)) as f:
            for line in f:
                if line.startswith(field):
                    return int(line.split()[1])
    except OSError:
        pass
    return 0


def uss_kb(pid):
    def field(prefix):
        try:
            with open(f"/proc/{pid}/smaps_rollup") as f:
                for line in f:
                    if line.startswith(prefix):
                        return int(line.split()[1])
        except OSError:
            pass
        return 0
    return field("Private_Clean:") + field("Private_Dirty:")


class Sampler:
    def __init__(self, pids_fn):
        self.pids_fn = pids_fn
        self.peak = 0
        self.pss_at_peak = 0
        self.uss_at_peak = 0
        self.hwm = 0
        self.stop = threading.Event()
        self.thread = threading.Thread(target=self._loop, daemon=True)

    def _loop(self):
        while not self.stop.is_set():
            try:
                pids = proc_tree(self.pids_fn())
                rss = sum(read_kb(p, "/proc/{pid}/status", "VmRSS:")
                          for p in pids)
                pss = sum(read_kb(p, "/proc/{pid}/smaps_rollup", "Pss:")
                          for p in pids)
                uss = sum(uss_kb(p) for p in pids)
                if rss > self.peak:
                    self.peak = rss
                    self.pss_at_peak = pss
                    self.uss_at_peak = uss
                self.hwm = max(self.hwm,
                               sum(read_kb(p, "/proc/{pid}/status", "VmHWM:")
                                   for p in pids))
            except Exception:
                pass
            time.sleep(0.25)

    def __enter__(self):
        self.thread.start()
        return self

    def __exit__(self, *a):
        self.stop.set()
        self.thread.join(timeout=2)


def measure(settle_ms, timeout_ms):
    out = pathlib.Path("/tmp/p11-diag.json")
    if out.exists():
        out.unlink()
    bin_ = ROOT / "target/release/brows-perf"
    cmd = [str(bin_), "--url", URL,
           "--width", "1280", "--height", "800",
           "--settle-ms", str(settle_ms), "--timeout-ms", str(timeout_ms),
           "--json", str(out)]
    proc = subprocess.Popen(cmd, env=GL_ENV,
                            stdout=subprocess.DEVNULL,
                            stderr=subprocess.DEVNULL)
    with Sampler(lambda: [proc.pid]) as s:
        try:
            proc.wait(timeout=(timeout_ms + settle_ms) / 1000 + 60)
        except subprocess.TimeoutExpired:
            proc.kill()
            return None
    if not out.exists() or proc.returncode != 0:
        return None
    r = json.loads(out.read_text())
    return {
        "rss_kb": s.peak, "pss_kb": s.pss_at_peak, "uss_kb": s.uss_at_peak,
        "peak_kb": max(s.hwm, s.peak), "observed_peak_kb": s.peak,
        "perf": r,
    }


def main():
    global URL
    ap = argparse.ArgumentParser()
    ap.add_argument("--out",
                    default="docs/perf-artifacts/v21/p11_cnn_diag.json")
    ap.add_argument("--settle-ms", type=int, default=6000)
    ap.add_argument("--timeout-ms", type=int, default=90000)
    ap.add_argument("--reps", type=int, default=1,
                    help="measurements per stage; worst peak is kept")
    ap.add_argument("--stage", default=None,
                    help="run a single stage name (default: all)")
    ap.add_argument("--url", default=URL,
                    help="override the measured URL")
    ap.add_argument("--resume", action="store_true")
    args = ap.parse_args()

    URL = args.url
    out_path = ROOT / args.out
    out_path.parent.mkdir(parents=True, exist_ok=True)
    results = {}
    if args.resume and out_path.exists():
        results = json.loads(out_path.read_text()).get("stages", {})

    for name, deny in STAGES:
        if args.stage and name != args.stage:
            continue
        if name in results and not args.stage:
            print(f"[skip] {name} already present")
            continue
        env = dict(GL_ENV)
        if deny:
            env["BROWS12_DIAG_DENY"] = deny
        best = None
        for rep in range(args.reps):
            os.environ.pop("BROWS12_DIAG_DENY", None)
            if deny:
                os.environ["BROWS12_DIAG_DENY"] = deny
            GL_ENV.pop("BROWS12_DIAG_DENY", None)
            r = measure(args.settle_ms, args.timeout_ms)
            if r is None:
                print(f"[warn] {name} rep {rep}: measurement failed")
                continue
            if best is None or r["peak_kb"] > best["peak_kb"]:
                best = r
            print(f"[ok] {name} rep {rep}: peak {r['peak_kb']/1024:.1f} MB")
        if best is None:
            print(f"[fail] stage {name}")
            sys.exit(1)
        results[name] = best
        json.dump({"url": URL, "stages": results}, out_path.open("w"),
                  indent=1)
        time.sleep(2)

    # Attribute: deltas vs full.
    if "full" in results:
        base = results["full"]["peak_kb"]
        attrib = {}
        for name, _ in STAGES:
            if name == "full" or name not in results:
                continue
            d = base - results[name]["peak_kb"]
            attrib[name] = {
                "delta_kb": d,
                "delta_mb": round(d / 1024, 1),
                "pct_of_full": round(100 * d / base, 1) if base else None,
            }
        json.dump({"url": URL, "stages": results, "attribution": attrib},
                  out_path.open("w"), indent=1)
        print(json.dumps(attrib, indent=1))


if __name__ == "__main__":
    main()
