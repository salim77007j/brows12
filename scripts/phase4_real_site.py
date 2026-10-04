#!/usr/bin/env python3
"""Phase 4 real-site side-by-side captures.

Usage: python3 scripts/phase4_real_site.py <name> <url> [<name> <url> ...]

For each site:
  1. Capture Chromium (Playwright) ground truth -> validation/run/phase4/ref/<name>.png
  2. Capture brows12 (brows-servo headless)     -> validation/run/phase4/ours/<name>.png
  3. Composite side-by-side                     -> screenshots/v2-servo/phase4/<name>_compare.png
"""
import http  # noqa: F401  (only to fail fast on odd environments)
import json
import os
import pathlib
import subprocess
import sys

from PIL import Image, ImageDraw

ROOT = pathlib.Path(__file__).resolve().parent.parent
REF = ROOT / "validation" / "run" / "phase4" / "ref"
OURS = ROOT / "validation" / "run" / "phase4" / "ours"
COMPARE = ROOT / "screenshots" / "v2-servo" / "phase4"
W, H, LABEL_H = 1280, 800, 28

SERVO_ENV = {
    **os.environ,
    "DISPLAY": ":99",
    "LD_LIBRARY_PATH": os.path.expanduser("~/.local/gl/usr/lib/x86_64-linux-gnu"),
    "__EGL_VENDOR_LIBRARY_FILENAMES": os.path.expanduser(
        "~/.local/gl/usr/share/glvnd/egl_vendor.d/50_mesa.json"),
    "XDG_RUNTIME_DIR": "/tmp/xdg",
    "RUST_LOG": "error",
}


def chrome_shot(page_factory, url, out):
    browser = page_factory.launch(args=["--force-device-scale-factor=1"])
    try:
        page = browser.new_page(viewport={"width": W, "height": H})
        page.goto(url, wait_until="domcontentloaded", timeout=45000)
        page.wait_for_timeout(4000)
        page.screenshot(path=str(out))
    finally:
        browser.close()


def brows_shot(binary, url, out, json_out):
    cmd = [str(binary), "--url", url, "--width", str(W), "--height", str(H),
           "--png", str(out), "--json", str(json_out),
           "--timeout-ms", "45000", "--settle-ms", "3000"]
    r = subprocess.run(cmd, capture_output=True, text=True, timeout=180,
                       env=SERVO_ENV)
    if r.returncode != 0:
        print(f"    brows12 stderr: {r.stderr.strip()[:300]}")
    return r.returncode == 0


def compose(name, url):
    ref, ours = REF / f"{name}.png", OURS / f"{name}.png"
    if not ref.exists() or not ours.exists():
        print(f"    SKIP compose {name}")
        return
    a, b = Image.open(ref).convert("RGB"), Image.open(ours).convert("RGB")
    canvas = Image.new("RGB", (a.width + b.width + 12,
                               max(a.height, b.height) + LABEL_H), (24, 26, 27))
    d = ImageDraw.Draw(canvas)
    d.text((8, 7),
           f"{name} ({url}) — LEFT: Chromium | RIGHT: brows12 v2 (Servo)",
           fill=(255, 255, 255))
    canvas.paste(a, (0, LABEL_H))
    canvas.paste(b, (a.width + 12, LABEL_H))
    canvas.save(COMPARE / f"{name}_compare.png")


def main():
    pairs = sys.argv[1:]
    assert pairs, "need <name> <url> pairs"
    for d in (REF, OURS, COMPARE):
        d.mkdir(parents=True, exist_ok=True)
    binary = ROOT / "target" / "debug" / "brows-servo"
    results = {}
    from playwright.sync_api import sync_playwright
    with sync_playwright() as pw:
        for name, url in zip(pairs[0::2], pairs[1::2]):
            print(f"[{name}] {url}")
            print("  chrome…", end="", flush=True)
            try:
                chrome_shot(pw.chromium, url, REF / f"{name}.png")
                print(" ok")
            except Exception as e:
                results[name] = {"chrome_error": str(e)[:200]}
                print(f" ERR {e}")
                continue
            print("  brows12…", end="", flush=True)
            ok = brows_shot(binary, url, OURS / f"{name}.png",
                            ROOT / "validation" / "run" / "phase4" / f"{name}.json")
            print(" ok" if ok else " RENDER FAILED")
            results[name] = {"brows12_rendered": ok, "url": url}
            compose(name, url)
    out = ROOT / "validation" / "run" / "phase4" / "results.json"
    out.write_text(json.dumps(results, indent=1))
    print(f"results -> {out}")


if __name__ == "__main__":
    main()
