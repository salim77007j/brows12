#!/usr/bin/env python3
"""Phase 4.3.7 — honest RAM comparison: brows12 vs Chrome (Playwright
Chromium), same machine, same pages, RSS/PSS/USS/peak.

Pages: 5 deterministic local heavy pages (1 MB decoded per PNG; big DOM)
+ up to 5 real heavy sites (skipped with a note if the network fails).

Protocol per page:
- brows12: one `brows-perf` process per page (fresh engine, cold), 1280x800;
  poll RSS @250 ms; sample PSS/USS/VmHWM at the settle point.
- Chrome: one Playwright Chromium per page, viewport 1280x800; poll the
  process-tree RSS @250 ms; sample tree PSS/USS at the settle point.

Usage: python3 scripts/area3_compare.py --out docs/perf-artifacts/phase4/area3/area3_ram.json
"""
import argparse
import json
import os
import pathlib
import subprocess
import sys
import threading
import time
import urllib.request

ROOT = pathlib.Path(__file__).resolve().parent.parent
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

REAL_SITES = [
    ("https://en.wikipedia.org/wiki/Rust_(programming_language)", "wikipedia-rust"),
    ("https://github.com/servo/servo", "github-servo"),
    ("https://news.ycombinator.com/", "hackernews"),
    ("https://www.bbc.com/news", "bbc-news"),
    ("https://www.cnn.com/", "cnn"),
]


def gen_heavy(images, dom_nodes, name):
    """Deterministic heavy page; returns (path, decoded_mb_estimate)."""
    import random, struct, zlib
    d = pathlib.Path("/tmp/a3_compare_site")
    d.mkdir(parents=True, exist_ok=True)
    w = h = 512
    random.seed(42)
    raw = b"".join(b"\x00" + bytes(random.randrange(256) for _ in range(w * 3))
                   for _ in range(h))
    def chunk(tag, data):
        c = struct.pack(">I", len(data)) + tag + data
        return c + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)
    ihdr = struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0)
    body = (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", ihdr)
            + chunk(b"IDAT", zlib.compress(raw, 6)) + chunk(b"IEND", b""))
    for i in range(images):
        (d / f"img{i}.png").write_bytes(body)
    rows = "".join(
        f"<div class='row r{i}'><b>Node {i}</b><span>filler text for DOM "
        f"weight, paragraph {i} of {dom_nodes}.</span></div>\n"
        for i in range(dom_nodes))
    imgs = "".join(f"<img src='img{i}.png' loading='eager'>\n"
                   for i in range(images))
    page = f"heavy_{name}.html"
    (d / page).write_text(
        "<!doctype html><html><head><title>Heavy " + name + "</title></head>"
        f"<body><h1>Heavy {name}</h1>{imgs}{rows}</body></html>")
    return page, images  # decoded MB ≈ images (1 MB per PNG)


def wait_http(url, timeout=30):
    end = time.time() + timeout
    while time.time() < end:
        try:
            with urllib.request.urlopen(url, timeout=2) as r:
                if r.status == 200:
                    return True
        except Exception:
            time.sleep(0.3)
    return False


# ---- /proc sampling ----------------------------------------------------

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


def sum_tree(pids, path_tpl, field):
    return sum(read_kb(p, path_tpl, field) for p in proc_tree(pids))


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


def sum_tree_uss(pids):
    return sum(uss_kb(p) for p in proc_tree(pids))


class Sampler:
    """Polls a tree-sum of RSS; keeps the observed peak and the
    peak-time PSS/USS (smaps_rollup is cheap; reads must happen while
    the process is alive)."""

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
                rss = sum(pids and [read_kb(p, "/proc/{pid}/status", "VmRSS:")
                                    for p in pids])
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


# ---- per-page measurements ---------------------------------------------

def measure_brows12(url, settle_ms, timeout_ms):
    out = pathlib.Path("/tmp/a3-b12.json")
    if out.exists():
        out.unlink()
    bin = os.environ.get("BROWS12_PERF_BIN", str(ROOT / "target/release/brows-perf"))
    cmd = [str(bin), "--url", url,
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
    return {"rss_kb": s.peak, "pss_kb": s.pss_at_peak,
            "uss_kb": s.uss_at_peak,
            "peak_kb": max(s.hwm, s.peak), "observed_peak_kb": s.peak}


def measure_chrome(url, settle_ms, timeout_ms):
    from playwright.sync_api import sync_playwright
    result = {}

    def tree_pids():
        info = result.get("_pids")
        return info or []

    with sync_playwright() as pw:
        browser = pw.chromium.launch(
            args=["--force-device-scale-factor=1", "--disable-gpu"])
        ctx = browser.new_context(viewport={"width": 1280, "height": 800})
        page = ctx.new_page()
        result["_pids"] = _chromium_pids()
        with Sampler(tree_pids) as s:
            try:
                page.goto(url, wait_until="load", timeout=timeout_ms)
            except Exception as e:
                browser.close()
                return None
            time.sleep(settle_ms / 1000)
            pids = _chromium_pids()
            result["_pids"] = pids
            rss = sum_tree(pids, "/proc/{pid}/status", "VmRSS:")
            pss = sum_tree(pids, "/proc/{pid}/smaps_rollup", "Pss:")
            uss = sum_tree_uss(pids)
            hwm = sum_tree(pids, "/proc/{pid}/status", "VmHWM:")
            browser.close()
        return {"rss_kb": rss, "pss_kb": pss, "uss_kb": uss,
                "peak_kb": max(hwm, s.peak), "observed_peak_kb": s.peak}


def _chromium_pids():
    """All live chromium/chrome processes (psutil, name-based)."""
    import psutil
    pids = []
    for p in psutil.process_iter(["name"]):
        try:
            n = (p.info["name"] or "").lower()
            if "chrome" in n or "chromium" in n:
                pids.append(p.pid)
        except (psutil.NoSuchProcess, psutil.AccessDenied):
            pass
    return pids


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--out",
                    default="docs/perf-artifacts/phase4/area3/area3_ram.json")
    ap.add_argument("--settle-ms", type=int, default=6000)
    ap.add_argument("--timeout-ms", type=int, default=90000)
    ap.add_argument("--real", type=int, default=5)
    ap.add_argument("--resume", action="store_true",
                    help="skip pages already present in --out")
    args = ap.parse_args()

    done = {}
    if args.resume and os.path.exists(args.out):
        try:
            for r in json.load(open(args.out)).get("pages_done", []):
                done[r["page"]] = r
        except Exception:
            done = {}

    def _dump(path, rows):
        pathlib.Path(path).parent.mkdir(parents=True, exist_ok=True)
        with open(path, "w") as f:
            json.dump({"pages_done": rows}, f, indent=2)

    pages = []
    srv = None
    # 5 deterministic local heavy pages, ascending weight
    for i, (imgs, dom) in enumerate(
            [(10, 500), (20, 800), (30, 800), (40, 1200), (50, 1500)]):
        page, decoded_mb = gen_heavy(imgs, dom, f"local{i}")
        pages.append((f"local-{imgs}img-{dom}dom",
                      f"http://127.0.0.1:8128/{page}",
                      f"local heavy: {imgs} imgs (~{imgs} MB decoded) "
                      f"+ {dom} DOM nodes"))
    if wait_http("http://127.0.0.1:8128/" + pages[0][0].replace(
            "http://127.0.0.1:8128/", ""), 1) is None:
        pass
    srv = subprocess.Popen(
        [sys.executable, "-m", "http.server", "8128",
         "--directory", "/tmp/a3_compare_site"],
        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)

    # real sites: keep those that respond
    kept = 0
    for url, name in REAL_SITES:
        if kept >= args.real:
            break
        try:
            with urllib.request.urlopen(url, timeout=8) as r:
                ok = r.status == 200
        except Exception:
            ok = False
        if ok:
            pages.append((name, url, "real site"))
            kept += 1
        else:
            pages.append((name + "-SKIPPED", None,
                          "real site unreachable, skipped"))

    rows = list(done.values())
    for name, url, note in pages:
        if url is None:
            continue
        if name in done:
            print(f"[cmp] {name}: cached", flush=True)
            continue
        print(f"[cmp] {name}: brows12 ...", flush=True)
        b12 = measure_brows12(url, args.settle_ms, args.timeout_ms)
        print(f"[cmp] {name}: brows12 "
              f"{(b12 or {}).get('peak_kb', 0) // 1024} MB peak", flush=True)
        print(f"[cmp] {name}: chrome ...", flush=True)
        chrome = measure_chrome(url, args.settle_ms, args.timeout_ms)
        print(f"[cmp] {name}: chrome "
              f"{(chrome or {}).get('peak_kb', 0) // 1024} MB peak", flush=True)
        ratio = (chrome["peak_kb"] / b12["peak_kb"]) if (b12 and chrome) else None
        rows.append({
            "page": name, "note": note,
            "brows12": b12, "chrome": chrome,
            "peak_ratio_chrome_over_b12": round(ratio, 2) if ratio else None,
        })
        # incremental save: a chunked CI environment may kill us mid-run
        _dump(args.out, rows)

    if srv:
        srv.terminate()
    out = {
        "protocol": "one fresh engine per page; 1280x800; brows12 = "
                    "brows-perf headless (software GL, debug build — same "
                    "convention as Phase 2/3); Chrome = Playwright "
                    "Chromium; RSS/PSS/USS sampled at settle, peak = "
                    "VmHWM tree sum vs 250 ms polled maximum",
        "budget_target": "brows12 peak <= 0.5 x chrome peak per page",
        "pages": rows,
    }
    passed = sum(1 for r in rows
                 if r["peak_ratio_chrome_over_b12"]
                 and r["peak_ratio_chrome_over_b12"] >= 2.0)
    out["pages_meeting_2x"] = passed
    out["pages_total"] = len([r for r in rows if r["brows12"] and r["chrome"]])
    pathlib.Path(args.out).parent.mkdir(parents=True, exist_ok=True)
    with open(args.out, "w") as f:
        json.dump(out, f, indent=2)
    print(json.dumps(out, indent=2))


if __name__ == "__main__":
    main()
