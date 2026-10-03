#!/usr/bin/env python3
"""v1.0 real-world site suite: render 25+ sites in Chromium and brows12,
compose side-by-side comparisons into screenshots/v1-final/.

Usage: python3 scripts/v1_site_suite.py [site ...]
"""
import json
import pathlib
import subprocess
import sys
from concurrent.futures import ThreadPoolExecutor

from playwright.sync_api import sync_playwright

ROOT = pathlib.Path(__file__).resolve().parent.parent
OUT = ROOT / "screenshots" / "v1-final"
REF = ROOT / "validation" / "run" / "reference"
FIN = ROOT / "validation" / "run" / "final-v1"

W, H = 1280, 800
UA = ("Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 "
      "(KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36")

SITES = [
    ("example_com", "https://example.com"),
    ("wikipedia_rust", "https://en.wikipedia.org/wiki/Rust_(programming_language)"),
    ("wikipedia_main", "https://en.wikipedia.org/wiki/Main_Page"),
    ("github_home", "https://github.com"),
    ("hacker_news", "https://news.ycombinator.com"),
    ("rust_lang_org", "https://www.rust-lang.org"),
    ("mozilla_org", "https://www.mozilla.org"),
    ("google_com", "https://www.google.com"),
    ("duckduckgo", "https://duckduckgo.com"),
    ("bing_search", "https://www.bing.com/search?q=rust+programming+language"),
    ("mdn_fetch", "https://developer.mozilla.org/en-US/docs/Web/API/fetch"),
    ("stackoverflow", "https://stackoverflow.com/questions/1732348/regex-match-open-tags-except-xhtml-self-contained-tags"),
    ("react_dev", "https://react.dev"),
    ("vuejs_org", "https://vuejs.org"),
    ("svelte_dev", "https://svelte.dev"),
    ("tailwindcss", "https://tailwindcss.com"),
    ("threejs_org", "https://threejs.org"),
    ("youtube", "https://www.youtube.com"),
    ("reddit", "https://www.reddit.com"),
    ("amazon", "https://www.amazon.com"),
    ("cloudflare", "https://www.cloudflare.com"),
    ("vercel", "https://vercel.com"),
    ("stripe", "https://stripe.com"),
    ("linear_app", "https://linear.app"),
    ("arxiv", "https://arxiv.org"),
    ("crates_io", "https://crates.io"),
    ("docs_rs", "https://docs.rs"),
]


def chrome_refs(names):
    with sync_playwright() as p:
        browser = p.chromium.launch()
        ctx = browser.new_context(viewport={"width": W, "height": H}, user_agent=UA,
                                  device_scale_factor=1)
        page = ctx.new_page()
        for name in names:
            url = dict(SITES)[name]
            try:
                page.goto(url, wait_until="domcontentloaded", timeout=45000)
                try:
                    page.wait_for_load_state("networkidle", timeout=6000)
                except Exception:
                    pass
                page.wait_for_timeout(600)
                page.screenshot(path=str(REF / f"{name}.png"))
                print(f"  chrome {name}")
            except Exception as e:
                print(f"  chrome FAIL {name}: {e}")
        browser.close()


def brows_render(name, url):
    png = FIN / f"{name}.png"
    meta = FIN / f"{name}.json"
    try:
        r = subprocess.run(
            [str(ROOT / "target" / "debug" / "brows"), "render", "--url", url,
             "--png", str(png), "--json", str(meta)],
            capture_output=True, timeout=150)
        if png.exists():
            data = json.loads(meta.read_text()) if meta.exists() else {}
            ms = data.get("timing", {}).get("full_pipeline_ms")
            print(f"  brows12 {name} ok ({ms} ms)")
            return
    except subprocess.TimeoutExpired:
        pass
    print(f"  brows12 FAIL {name}")


def compose(name):
    ref = REF / f"{name}.png"
    fin = FIN / f"{name}.png"
    if not ref.exists() or not fin.exists():
        return
    from PIL import Image, ImageDraw
    a = Image.open(ref).convert("RGB")
    b = Image.open(fin).convert("RGB")
    h = max(a.height, b.height)
    w = a.width + b.width + 12
    c = Image.new("RGB", (w, h + 28), (24, 26, 27))
    d = ImageDraw.Draw(c)
    d.text((8, 7), f"{name} - LEFT: Chromium | RIGHT: brows12 v1.0.0-rc1", fill=(255, 255, 255))
    c.paste(a, (0, 28))
    c.paste(b, (a.width + 12, 28))
    c.save(OUT / f"{name}_compare.png")


def main():
    REF.mkdir(parents=True, exist_ok=True)
    FIN.mkdir(parents=True, exist_ok=True)
    OUT.mkdir(parents=True, exist_ok=True)
    names = sys.argv[1:] or [n for n, _ in SITES]
    chrome_refs(names)
    with ThreadPoolExecutor(max_workers=1) as pool:
        for n in names:
            brows_render(n, dict(SITES)[n])
    for n in names:
        compose(n)
    print("done ->", OUT)


if __name__ == "__main__":
    main()
