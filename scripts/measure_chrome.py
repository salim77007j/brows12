#!/usr/bin/env python3
"""Chrome (Playwright Chromium) comparison numbers for Phase 2:
- RAM for 1 / 10 tabs of example.com (sum of all chromium RSS)
- Idle CPU% over a window with 10 tabs
- Cold start: launch -> example.com loaded (domcontentloaded)

Usage: measure_chrome.py --tabs 10 --out out.json
"""
import argparse, json, os, time

from playwright.sync_api import sync_playwright

def chrome_pids(root_pid):
    """All descendant pids of the chromium process tree."""
    pids = [root_pid]
    i = 0
    while i < len(pids):
        pid = pids[i]
        i += 1
        try:
            for entry in os.listdir(f"/proc/{pid}/task"):
                with open(f"/proc/{pid}/task/{entry}/children") as f:
                    pids.extend(int(c) for c in f.read().split())
        except OSError:
            pass
    return pids

def rss_kb_all(pids):
    total = 0
    for pid in set(pids):
        try:
            with open(f"/proc/{pid}/status") as f:
                for line in f:
                    if line.startswith("VmRSS:"):
                        total += int(line.split()[1])
                        break
        except OSError:
            pass
    return total

def cpu_seconds_all(pids):
    total = 0.0
    hz = os.sysconf("SC_CLK_TCK")
    for pid in set(pids):
        try:
            with open(f"/proc/{pid}/stat", "rb") as f:
                parts = f.read().split()
            total += (int(parts[13]) + int(parts[14])) / hz
        except (OSError, IndexError, ValueError):
            pass
    return total

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--url", default="https://example.com/")
    ap.add_argument("--out", default="/tmp/b12-chrome.json")
    ap.add_argument("--idle-secs", type=float, default=20.0)
    args = ap.parse_args()

    results = {}
    with sync_playwright() as pw:
        browser = pw.chromium.launch(headless=True)

        # ---- 1 tab RAM + cold start ---------------------------------
        t0 = time.time()
        ctx = browser.new_context(viewport={"width": 1280, "height": 728})
        page = ctx.new_page()
        page.goto(args.url, wait_until="domcontentloaded", timeout=60_000)
        loaded_ms = (time.time() - t0) * 1000
        time.sleep(2.0)
        # Find the playwright-launched chromium processes: scan /proc for
        # chrome/headless_shell binaries whose cmdline references playwright.
        cand = []
        for entry in os.listdir("/proc"):
            if not entry.isdigit():
                continue
            try:
                with open(f"/proc/{entry}/comm") as f:
                    comm = f.read().strip()
                if "chrome" in comm or "headless" in comm:
                    with open(f"/proc/{entry}/cmdline") as f:
                        cmd = f.read()
                    if "playwright" in cmd or "ms-playwright" in cmd:
                        cand.append(int(entry))
            except OSError:
                pass
        pids1 = []
        for pid in cand:
            pids1.extend(chrome_pids(pid))
        rss_1tab = rss_kb_all(pids1)
        print(f"[chrome] cold start->loaded: {loaded_ms:.0f} ms; 1-tab RSS: {rss_1tab/1024:.1f} MB", flush=True)
        ctx.close()

        # ---- 10 tabs RAM + idle CPU ----------------------------------
        ctx = browser.new_context(viewport={"width": 1280, "height": 728})
        pages = [ctx.new_page() for _ in range(10)]
        for p in pages:
            p.goto(args.url, wait_until="domcontentloaded", timeout=60_000)
        time.sleep(3.0)
        pids10 = []
        for pid in cand:
            pids10.extend(chrome_pids(pid))
        rss_10tab = rss_kb_all(pids10)
        c0 = cpu_seconds_all(pids10)
        t0 = time.time()
        time.sleep(args.idle_secs)
        c1 = cpu_seconds_all(pids10)
        t1 = time.time()
        idle_cpu = 100.0 * (c1 - c0) / (t1 - t0)
        print(f"[chrome] 10-tab RSS: {rss_10tab/1024:.1f} MB; idle CPU {idle_cpu:.3f}% over {t1-t0:.0f}s", flush=True)
        ctx.close()
        browser.close()

    results = {
        "engine": "chromium (playwright headless_shell)",
        "url": args.url,
        "cold_start_loaded_ms": round(loaded_ms, 1),
        "rss_1tab_kb": rss_1tab,
        "rss_10tab_kb": rss_10tab,
        "rss_10tab_per_tab_kb": round(rss_10tab / 10, 1),
        "idle_cpu_percent_10tabs": round(idle_cpu, 3),
        "idle_window_secs": round(t1 - t0, 1),
    }
    with open(args.out, "w") as f:
        json.dump(results, f, indent=1)
    print(json.dumps(results, indent=1))

if __name__ == "__main__":
    main()
