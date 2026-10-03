#!/usr/bin/env python3
"""Chrome ground truth for the Phase 3.9 heavy page:
RSS (renderer processes) + rAF FPS while auto-scrolling.

Usage: python3 scripts/phase3_heavy_chrome.py [out.json]
"""
import json
import pathlib
import subprocess
import sys
import time

import psutil
from playwright.sync_api import sync_playwright

OUT = pathlib.Path(sys.argv[1] if len(sys.argv) > 1
                   else "docs/perf-artifacts/phase3/heavy_chrome.json")
URL = "http://127.0.0.1:8125/index.html"


def chrome_tree_rss_kb():
    total = 0
    for p in psutil.process_iter(["name", "memory_info"]):
        try:
            if "chrome" in (p.info["name"] or "").lower():
                total += p.info["memory_info"].rss / 1024
        except (psutil.NoSuchProcess, psutil.AccessDenied):
            pass
    return int(total)


def main():
    results = {}
    with sync_playwright() as pw:
        browser = pw.chromium.launch(
            args=["--force-device-scale-factor=1", "--disable-gpu"])
        page = browser.new_page(viewport={"width": 1280, "height": 800})
        before = chrome_tree_rss_kb()
        page.goto(URL, wait_until="load", timeout=120000)
        page.wait_for_timeout(4000)  # decode/settle
        results["rss_after_load_kb"] = chrome_tree_rss_kb()
        results["rss_baseline_before_kb"] = before
        results["rss_marginal_kb"] = results["rss_after_load_kb"] - before

        # rAF FPS while scrolling for 6s (same window as brows-perf).
        fps = page.evaluate("""async () => {
          let frames = 0;
          let run = true;
          const tick = () => { if (!run) return; frames++; window.scrollBy(0, 60);
                               requestAnimationFrame(tick); };
          requestAnimationFrame(tick);
          const t0 = performance.now();
          await new Promise(r => setTimeout(r, 6000));
          run = false;
          return { frames, secs: (performance.now() - t0) / 1000,
                   scrollY: window.scrollY };
        }""")
        results["scroll"] = fps
        results["scroll_fps"] = fps["frames"] / fps["secs"]
        browser.close()

    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text(json.dumps(results, indent=2))
    print(json.dumps(results, indent=2))


if __name__ == "__main__":
    main()
