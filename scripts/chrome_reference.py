#!/usr/bin/env python3
"""Capture Chrome ground-truth screenshots with Playwright Chromium.

Usage:
  python3 scripts/chrome_reference.py                 # capture all SITES
  python3 scripts/chrome_reference.py url out.png     # capture one URL

Output: validation/run/reference/<name>.png (1280x800, matching the
harness default viewport so side-by-side comparisons line up).
"""
import sys
import pathlib
from playwright.sync_api import sync_playwright

ROOT = pathlib.Path(__file__).resolve().parent.parent
OUT = ROOT / "validation" / "run" / "reference"

W, H = 1280, 800

SITES = [
    ("wikipedia_rust", "https://en.wikipedia.org/wiki/Rust_(programming_language)"),
    ("rust_lang_org", "https://www.rust-lang.org"),
    ("github_home", "https://github.com"),
    ("hacker_news", "https://news.ycombinator.com"),
    ("example_com", "https://example.com"),
    ("bing_search", "https://www.bing.com/search?q=rust+programming+language"),
]

UA = ("Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 "
      "(KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36")


def capture(page, url, path):
    page.goto(url, wait_until="domcontentloaded", timeout=45000)
    try:
        page.wait_for_load_state("networkidle", timeout=8000)
    except Exception:
        pass  # pages with long-polling never go idle
    page.wait_for_timeout(700)
    page.screenshot(path=str(path))
    print(f"  {path.name}  <-  {url}")


def main():
    OUT.mkdir(parents=True, exist_ok=True)
    targets = []
    if len(sys.argv) == 3:
        url, out = sys.argv[1], sys.argv[2]
        targets.append((pathlib.Path(out).stem, url))
    else:
        targets = SITES

    with sync_playwright() as p:
        browser = p.chromium.launch()
        ctx = browser.new_context(viewport={"width": W, "height": H},
                                  user_agent=UA, device_scale_factor=1)
        page = ctx.new_page()
        for name, url in targets:
            try:
                capture(page, url, OUT / f"{name}.png")
            except Exception as e:
                print(f"  FAIL {name}: {e}")
        browser.close()


if __name__ == "__main__":
    main()
