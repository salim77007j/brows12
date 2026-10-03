#!/usr/bin/env python3
"""Phase 3 real-world site captures: brows12 vs Chromium side-by-side.

Sites exercise live shadow DOM (github, wpt.fyi), WOFF2 web fonts
(rust-lang.org) and a plain-DOM control (Hacker News).

Usage: python3 scripts/phase3_sites.py [name ...]
"""
import json
import os
import pathlib
import subprocess
import sys

from PIL import Image, ImageDraw

ROOT = pathlib.Path(__file__).resolve().parent.parent
REF = ROOT / "validation" / "run" / "phase3" / "ref"
OURS = ROOT / "validation" / "run" / "phase3" / "ours"
COMPARE = ROOT / "screenshots" / "v2-servo" / "phase3"
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

SITES = [
    ("rw_github_home", "https://github.com/"),
    ("rw_wpt_fyi", "https://wpt.fyi/results"),
    ("rw_rust_lang", "https://www.rust-lang.org/"),
    ("rw_hacker_news", "https://news.ycombinator.com/"),
]


def main():
    names = sys.argv[1:]
    sites = [s for s in SITES if not names or s[0] in names]
    for d in (REF, OURS, COMPARE):
        d.mkdir(parents=True, exist_ok=True)
    binary = ROOT / "target" / "debug" / "brows-servo"

    from playwright.sync_api import sync_playwright
    results = {}
    with sync_playwright() as pw:
        browser = pw.chromium.launch(args=["--force-device-scale-factor=1"])
        page = browser.new_page(viewport={"width": W, "height": H})
        for name, url in sites:
            print(f"[{name}] {url}")
            print("  chrome…", end="", flush=True)
            try:
                page.goto(url, wait_until="domcontentloaded", timeout=45000)
                try:
                    page.wait_for_load_state("networkidle", timeout=10000)
                except Exception:
                    pass
                page.wait_for_timeout(1200)
                page.screenshot(path=str(REF / f"{name}.png"))
                print(" ok")
            except Exception as e:
                results[name] = {"chrome_error": str(e)[:200]}
                print(f" ERR {e}")
                continue

            print("  brows12…", end="", flush=True)
            r = subprocess.run(
                [str(binary), "--url", url, "--width", str(W), "--height", str(H),
                 "--png", str(OURS / f"{name}.png"),
                 "--json", str(ROOT / "validation" / "run" / "phase3" / f"{name}.json"),
                 "--timeout-ms", "60000", "--settle-ms", "4000"],
                capture_output=True, text=True, timeout=150, env=SERVO_ENV)
            print(" ok" if r.returncode == 0 else f" FAILED ({r.stderr[-160:]})")
            results[name] = {"brows12_ok": r.returncode == 0,
                             "brows12_json": json.loads(
                                 (ROOT / "validation" / "run" / "phase3" / f"{name}.json").read_text()
                             ) if r.returncode == 0 else None}

            if (REF / f"{name}.png").exists() and (OURS / f"{name}.png").exists():
                a = Image.open(REF / f"{name}.png").convert("RGB")
                b = Image.open(OURS / f"{name}.png").convert("RGB")
                canvas = Image.new("RGB", (a.width + b.width + 12,
                                           max(a.height, b.height) + LABEL_H),
                                   (24, 26, 27))
                d = ImageDraw.Draw(canvas)
                d.text((8, 7), f"{name} — LEFT: Chromium | RIGHT: brows12 v2 (Servo)",
                       fill=(255, 255, 255))
                canvas.paste(a, (0, LABEL_H))
                canvas.paste(b, (a.width + 12, LABEL_H))
                canvas.save(COMPARE / f"{name}_compare.png")
                print(f"  -> {COMPARE / (name + '_compare.png')}")
        browser.close()

    out = ROOT / "validation" / "run" / "phase3" / "sites_results.json"
    out.write_text(json.dumps(results, indent=2, default=str))
    print(f"results -> {out.relative_to(ROOT)}")


if __name__ == "__main__":
    main()
