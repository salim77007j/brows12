#!/usr/bin/env python3
"""Phase 3 side-by-side verifier.

For every fixture in fixtures/phase3/:
  1. Serve it over http://127.0.0.1:8123 (same URL for both engines).
  2. Capture Chromium (Playwright) ground truth  -> validation/run/phase3/ref/<name>.png
  3. Capture brows12 (brows-servo headless)      -> validation/run/phase3/ours/<name>.png
  4. Composite side-by-side                      -> screenshots/v2-servo/phase3/<name>_compare.png

Usage:
  python3 scripts/phase3_run.py                 # all fixtures
  python3 scripts/phase3_run.py shadow_dom_basic css_columns   # subset
"""
import http.server
import json
import os
import socketserver
import subprocess
import sys
import threading
import pathlib
import functools

from PIL import Image, ImageDraw

ROOT = pathlib.Path(__file__).resolve().parent.parent
# brows-servo needs the user-local Mesa EGL stack + Xvfb (same env as the
# Phase 2 scripts — see scripts/validate_phase2.py XVFB_ENV).
SERVO_ENV = {
    **os.environ,
    "DISPLAY": ":99",
    "LD_LIBRARY_PATH": os.path.expanduser("~/.local/gl/usr/lib/x86_64-linux-gnu"),
    "__EGL_VENDOR_LIBRARY_FILENAMES": os.path.expanduser(
        "~/.local/gl/usr/share/glvnd/egl_vendor.d/50_mesa.json"),
    "XDG_RUNTIME_DIR": "/tmp/xdg",
    "RUST_LOG": "error",
}
FIXTURES = ROOT / "fixtures" / "phase3"
REF = ROOT / "validation" / "run" / "phase3" / "ref"
OURS = ROOT / "validation" / "run" / "phase3" / "ours"
COMPARE = ROOT / "screenshots" / "v2-servo" / "phase3"
PORT = 8123
W, H, LABEL_H = 1280, 800, 28

ALL = [
    "shadow_dom_basic", "shadow_dom_slots",
    "grid_areas", "grid_auto_fill", "grid_subgrid",
    "css_sticky_fixed", "css_transforms_anim", "css_shadows_radius",
    "css_backgrounds", "css_math_functions", "css_custom_props",
    "css_filters_clip", "css_writing_modes", "css_columns",
    "webfonts", "canvas2d", "gpu_apis", "webrtc_probe", "wasm",
]


def serve():
    handler = functools.partial(http.server.SimpleHTTPRequestHandler,
                                directory=str(FIXTURES))
    socketserver.TCPServer.allow_reuse_address = True
    srv = socketserver.TCPServer(("127.0.0.1", PORT), handler)
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    return srv


def chrome_shot(page_factory, url, out):
    """Playwright chromium capture (deterministic waits)."""
    browser = page_factory.launch(args=["--force-device-scale-factor=1"])
    try:
        page = browser.new_page(viewport={"width": W, "height": H})
        page.goto(url, wait_until="networkidle", timeout=30000)
        page.wait_for_timeout(1500)  # let runtime-update fixtures settle
        page.screenshot(path=str(out))
        # Grab the DOM self-report text for the verdict record
        report = ""
        try:
            report = page.inner_text("#report")
        except Exception:
            pass
        return report
    finally:
        browser.close()


def brows_shot(binary, url, out, json_out):
    cmd = [str(binary), "--url", url, "--width", str(W), "--height", str(H),
           "--png", str(out), "--json", str(json_out),
           "--timeout-ms", "45000", "--settle-ms", "2600"]
    r = subprocess.run(cmd, capture_output=True, text=True, timeout=120,
                       env=SERVO_ENV)
    if r.returncode != 0:
        print(f"    brows12 stderr: {r.stderr.strip()[:300]}")
    return r.returncode == 0


def compose(name):
    ref, ours = REF / f"{name}.png", OURS / f"{name}.png"
    if not ref.exists() or not ours.exists():
        print(f"    SKIP compose {name}")
        return
    a, b = Image.open(ref).convert("RGB"), Image.open(ours).convert("RGB")
    canvas = Image.new("RGB", (a.width + b.width + 12,
                               max(a.height, b.height) + LABEL_H), (24, 26, 27))
    d = ImageDraw.Draw(canvas)
    d.text((8, 7),
           f"{name} — LEFT: Chromium (ground truth) | RIGHT: brows12 v2 (Servo)",
           fill=(255, 255, 255))
    canvas.paste(a, (0, LABEL_H))
    canvas.paste(b, (a.width + 12, LABEL_H))
    canvas.save(COMPARE / f"{name}_compare.png")


def main():
    names = sys.argv[1:] or ALL
    for d in (REF, OURS, COMPARE):
        d.mkdir(parents=True, exist_ok=True)

    binary = pathlib.Path(os.environ.get("BROWS12_SERVO_BIN", ROOT / "target" / "release" / "brows-servo"))  # v2.1: release (debug dropped for disk)
    srv = serve()
    results = {}
    try:
        from playwright.sync_api import sync_playwright
        with sync_playwright() as pw:
          pw_chrome = pw.chromium
          for name in names:
                url = f"http://127.0.0.1:{PORT}/{name}.html"
                print(f"[{name}]")
                print("  chrome…", end="", flush=True)
                try:
                    report = chrome_shot(pw_chrome, url, REF / f"{name}.png")
                    results[name] = {"chrome_report": report.strip().splitlines()[:24]}
                    print(" ok")
                except Exception as e:
                    results[name] = {"chrome_error": str(e)[:200]}
                    print(f" ERR {e}")
                print("  brows12…", end="", flush=True)
                ok = brows_shot(binary, url, OURS / f"{name}.png",
                                ROOT / "validation" / "run" / "phase3" / f"{name}.json")
                print(" ok" if ok else " RENDER FAILED")
                results[name]["brows12_rendered"] = ok
                compose(name)
    finally:
        srv.shutdown()

    out = ROOT / "validation" / "run" / "phase3" / "run_results.json"
    out.write_text(json.dumps(results, indent=2))
    print(f"\nresults -> {out.relative_to(ROOT)}")


if __name__ == "__main__":
    main()
