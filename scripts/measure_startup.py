#!/usr/bin/env python3
"""Measure UI cold start: launch brows12-ui, read BROWS12_UI_START_METRICS
(servo_build_ms + first_present_ms), then quit. N repetitions.

Usage: measure_startup.py --bin PATH --reps 5 --out out.json
"""
import argparse, json, os, subprocess, threading, time, statistics

ENV = {
    **os.environ,
    "DISPLAY": ":99",
    "LD_LIBRARY_PATH": os.path.expanduser("~/.local/gl/usr/lib/x86_64-linux-gnu"),
    "__EGL_VENDOR_LIBRARY_FILENAMES": os.path.expanduser(
        "~/.local/gl/usr/share/glvnd/egl_vendor.d/50_mesa.json"),
    "XDG_RUNTIME_DIR": "/tmp/xdg",
    "RUST_LOG": "error",
}

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--bin", required=True)
    ap.add_argument("--reps", type=int, default=5)
    ap.add_argument("--out", default="/tmp/b12-startup.json")
    args = ap.parse_args()

    d = "/tmp/b12-start"
    os.makedirs(d, exist_ok=True)
    runs = []
    for i in range(args.reps):
        metrics_path = f"{d}/start_metrics.json"
        for f in os.listdir(d):
            os.unlink(os.path.join(d, f))
        env = {**ENV, "BROWS12_UI_START_METRICS": metrics_path}
        t0 = time.time()
        proc = subprocess.Popen([args.bin], env=env,
                                stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        # Wait for the metrics file (written on first present).
        deadline = time.time() + 60
        ok = False
        while time.time() < deadline:
            if os.path.exists(metrics_path):
                try:
                    m = json.load(open(metrics_path))
                    if m.get("first_present_ms") is not None:
                        ok = True
                        break
                except (json.JSONDecodeError, OSError):
                    pass
            time.sleep(0.02)
        wall_to_present = (time.time() - t0) * 1000
        proc.terminate()
        try:
            proc.wait(timeout=8)
        except subprocess.TimeoutExpired:
            proc.kill()
        if ok:
            m["wall_to_present_ms"] = round(wall_to_present, 1)
            runs.append(m)
            print(f"[startup] rep {i+1}: servo_build={m['servo_build_ms']}ms "
                  f"first_present={m['first_present_ms']}ms wall={m['wall_to_present_ms']}ms", flush=True)
        else:
            print(f"[startup] rep {i+1}: TIMEOUT", flush=True)
        time.sleep(1.0)

    summary = {}
    if runs:
        fp = [r["first_present_ms"] for r in runs]
        sb = [r["servo_build_ms"] for r in runs]
        summary = {
            "reps": len(runs),
            "first_present_ms_median": statistics.median(fp),
            "first_present_ms_min": min(fp),
            "first_present_ms_max": max(fp),
            "servo_build_ms_median": statistics.median(sb),
        }
        print(json.dumps(summary, indent=1))
    with open(args.out, "w") as f:
        json.dump({"runs": runs, "summary": summary}, f, indent=1)

if __name__ == "__main__":
    main()
