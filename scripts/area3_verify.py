#!/usr/bin/env python3
"""Phase 4 Area 3 verification harness (sub-items 3.1, 3.4, 3.2, 3.5).

Run A (3.1+3.4): 6 tabs (3 heavy local + 3 example.com) with a tight
governor budget -> expect hibernations, per-tab trims and a js_heap_tier
escalation in the event stream; RSS must drop vs peak.

Run B (3.2): two image-heavy tabs + one active; hibernate the two heavy
tabs by command; RSS delta = decoded-image memory returned to the OS.

Run C (3.5): same as B but compares against a never-opened baseline ->
residual = memory the engine cannot reclaim on hibernation (the Phase 3
WebRender texture-cache gap, now quantified).

Usage: python3 scripts/area3_verify.py --out docs/perf-artifacts/phase4/area3/area3_verify.json
"""
import argparse
import json
import os
import pathlib
import re
import subprocess
import sys
import threading
import time
import urllib.request

ROOT = pathlib.Path(__file__).resolve().parent.parent
XVFB_ENV = {
    **os.environ,
    "DISPLAY": ":99",
    "LD_LIBRARY_PATH": os.path.expanduser(
        "~/.local/gl/usr/lib/x86_64-linux-gnu"),
    "__EGL_VENDOR_LIBRARY_FILENAMES": os.path.expanduser(
        "~/.local/gl/usr/share/glvnd/egl_vendor.d/50_mesa.json"),
    "LIBGL_ALWAYS_SOFTWARE": "1",
    "XDG_RUNTIME_DIR": "/tmp/xdg",
    "RUST_LOG": "error",
}


def read_status_field(pid, field):
    try:
        with open(f"/proc/{pid}/status") as f:
            for line in f:
                if line.startswith(field):
                    return int(line.split()[1])
    except OSError:
        pass
    return 0


def rss_kb(pid):
    return read_status_field(pid, "VmRSS:")


def peak_kb(pid):
    return read_status_field(pid, "VmHWM:")


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


def gen_heavy_site(images=80, dom_nodes=1500, page_name="index.html",
                   out_dir="/tmp/heavy_site_a3"):
    """Heavy page: big DOM + N PNGs (1 MB decoded each). Local stdlib only."""
    import random, struct, zlib
    d = pathlib.Path(out_dir)
    d.mkdir(parents=True, exist_ok=True)
    png = d / "img0.png"
    if not png.exists():
        random.seed(42)
        w = h = 512
        raw = b""
        for y in range(h):
            raw += b"\x00" + bytes(random.randrange(256)
                                   for _ in range(w * 3))
        def chunk(tag, data):
            c = struct.pack(">I", len(data)) + tag + data
            return c + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)
        ihdr = struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0)
        body = (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", ihdr)
                + chunk(b"IDAT", zlib.compress(raw, 6)) + chunk(b"IEND", b""))
        for i in range(images):
            (d / f"img{i}.png").write_bytes(body)
        rows = "".join(
            f"<div class='row r{i}'><b>Node {i}</b> "
            f"<span>filler text for DOM weight, paragraph {i} of "
            f"{dom_nodes}.</span></div>\n"
            for i in range(dom_nodes))
        imgs = "".join(f"<img src='img{i}.png' loading='eager'>\n"
                       for i in range(images))
        (d / page_name).write_text(
            "<!doctype html><html><head><title>Area3 Heavy "
            f"{images}img/{dom_nodes}dom</title></head><body>"
            f"<h1>Heavy page A3</h1>{imgs}{rows}"
            "<script>setInterval(()=>{document.title='A3 tick '"
            "+Date.now()%1000;},2000);</script></body></html>")
        (d / "light.html").write_text(
            "<!doctype html><html><head><title>A3 light</title></head>"
            "<body><h1>Light control page</h1><p>No media weight.</p>"
            "</body></html>")
    if not (d / "light.html").exists():
        (d / "light.html").write_text(
            "<!doctype html><html><head><title>A3 light</title></head>"
            "<body><h1>Light control page</h1><p>No media weight.</p>"
            "</body></html>")
    return d


class UiRun:
    """One brows12-ui automation session under Xvfb."""

    def __init__(self, env_extra, name):
        self.name = name
        self.dir = pathlib.Path(f"/tmp/a3-{name}")
        self.dir.mkdir(parents=True, exist_ok=True)
        for f in self.dir.iterdir():
            f.unlink()
        self.cmd_fifo = str(self.dir / "cmd")
        self.evt_fifo = str(self.dir / "evt")
        os.mkfifo(self.cmd_fifo)
        os.mkfifo(self.evt_fifo)
        self.events = []
        self.proc = subprocess.Popen(
            [str(ROOT / "target/debug/brows12-ui")],
            env={**XVFB_ENV, "BROWS12_UI_CMD_FIFO": self.cmd_fifo,
                 "BROWS12_UI_EVENT_FIFO": self.evt_fifo, **env_extra},
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        self.reader = threading.Thread(target=self._read, daemon=True)
        self.reader.start()
        self.w = open(self.cmd_fifo, "w")

    def _read(self):
        with open(self.evt_fifo) as f:
            for line in f:
                self.events.append(line.strip())
                if line.startswith("quit"):
                    return

    def cmd(self, line):
        self.w.write(line + "\n")
        self.w.flush()

    def wait_for(self, pred, timeout=90, what="event"):
        end = time.time() + timeout
        while time.time() < end:
            for e in self.events:
                if pred(e):
                    return e
            time.sleep(0.1)
        raise TimeoutError(f"{what} ({self.name})")

    def rss(self):
        return rss_kb(self.proc.pid)

    def peak(self):
        return peak_kb(self.proc.pid)

    def close(self):
        try:
            self.cmd("<QUIT>")
            self.proc.wait(timeout=15)
        except Exception:
            self.proc.kill()


def open_tabs(run, urls, settle=6, heavy_wait=1):
    """<NEWTAB> + <OMNI> url + <RETURN> per tab. Each real navigation
    emits `loaded tab= url=...` twice (premature + real, per Phase 2).
    Heavy pages in the debug build decode slowly, so `heavy_wait=1`
    loaded event + the caller's settle window is enough for them."""
    for u in urls:
        # distinctive substring for matching loaded events
        if "light.html" in u:
            marker = "light.html"
            need = 2
        else:
            marker = u.rsplit("/", 1)[-1]  # index.html?v=N
            need = heavy_wait
        run.cmd("<NEWTAB>")
        time.sleep(0.4)
        run.cmd(f"<OMNI> {u}")
        time.sleep(0.2)
        run.cmd("<RETURN>")
        end = time.time() + 240
        while time.time() < end:
            n = sum(1 for e in run.events
                    if e.startswith("loaded tab=") and marker in e)
            if n >= need:
                break
            time.sleep(0.25)
        else:
            raise TimeoutError(f"load {u} ({run.name}): "
                               f"{run.events[-5:]}")
    time.sleep(settle)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--out",
                    default="docs/perf-artifacts/phase4/area3/area3_verify.json")
    ap.add_argument("--skip-runs", default="")
    args = ap.parse_args()
    out = {}

    # ---- Xvfb ---------------------------------------------------------
    xvfb = subprocess.Popen(["Xvfb", ":99", "-screen", "0", "1280x800x24"],
                            stdout=subprocess.DEVNULL,
                            stderr=subprocess.DEVNULL)
    time.sleep(1.5)

    heavy = gen_heavy_site(images=20, dom_nodes=800)
    # debug build: 20 imgs x 1MB decoded per tab keeps loads inside the
    # timeout budget while still dominating RSS (Phase 2/3 convention:
    # debug-build measurements, honestly labeled)
    srv = subprocess.Popen(
        [sys.executable, "-m", "http.server", "8125",
         "--directory", str(heavy)],
        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    assert wait_http("http://127.0.0.1:8125/index.html"), "http server"

    heavy_url = "http://127.0.0.1:8125/index.html"
    heavy_urls = [heavy_url + f"?v={i}" for i in range(3)]
    # the server ignores ?v= but each tab still loads the full page

    # ---- Run A: governor under tight budget (3.1 + 3.4) ---------------
    if "a" not in args.skip_runs:
        env = {
            "BROWS12_MEM_BUDGET_MB": "110",   # tight: 2 heavy tabs ≈ >110 MB
            "BROWS12_GOVERNOR_INTERVAL_MS": "2000",
            "BROWS12_TAB_SUSPEND_SECS": "8",
            "BROWS12_HEAVY_SUSPEND_SECS": "4",
            "BROWS12_PSI_COOLDOWN_MS": "4000",
        }
        run = UiRun(env, "gov")
        try:
            run.wait_for(lambda e: e.startswith("start "), 60, "start")
            open_tabs(run, heavy_urls[:2] + ["http://127.0.0.1:8125/light.html"] * 2)
            peak_before = run.peak()
            time.sleep(30)  # let the governor tick at least 10 times
            events = list(run.events)
            out["run_a_governor"] = {
                "budget_mb": 110,
                "peak_rss_mb": round(peak_kb(run.proc.pid) / 1024, 1),
                "rss_after_mb": round(run.rss() / 1024, 1),
                "hibernate_events": len([e for e in events
                                         if e.startswith("hibernate tab")]),
                "trim_events": len([e for e in events
                                    if e.startswith("tab_trim")]),
                "budget_hibernate_events": len(
                    [e for e in events
                     if e.startswith("tab_budget_hibernate")]),
                "js_tier_events": [e for e in events
                                   if e.startswith("js_heap_tier")][:4],
                "governor_global_events": [e for e in events
                                           if e.startswith("governor_global")][:4],
                "tier_escalated": any("tier=Tight" in e or "tier=Small" in e
                                      or "tier=Minimal" in e
                                      for e in events),
            }
        finally:
            run.close()

    # ---- Run B: image eviction on hibernate (3.2) ----------------------
    # ---- Run C: texture-cache residual (3.5) ---------------------------
    if "b" not in args.skip_runs:
        env = {
            "BROWS12_GOVERNOR": "0",   # manual control only
            "BROWS12_TAB_SUSPEND_SECS": "3600",
        }
        run = UiRun(env, "img")
        try:
            run.wait_for(lambda e: e.startswith("start "), 60, "start")
            # baseline: start page only
            time.sleep(4)
            baseline_rss = run.rss()
            # open two image-heavy tabs (20 imgs x 1MB decoded each)
            open_tabs(run, [heavy_url, heavy_url + "?v=1"], settle=8)
            loaded_rss = run.rss()
            # active = tab 0; switch back so BOTH are background... we
            # need a light active tab: open example.com as third tab and
            # stay there.
            run.cmd("<NEWTAB>")
            time.sleep(0.4)
            run.cmd("<OMNI> http://127.0.0.1:8125/light.html")
            time.sleep(0.2)
            run.cmd("<RETURN>")
            run.wait_for(lambda e: e.startswith("loaded tab=")
                         and "light.html" in e, 120, "light load")
            time.sleep(5)
            with_heavy_bg = run.rss()
            # hibernate the two heavy tabs (indices 0 and 1)
            run.cmd("<HIBERNATE> 0")
            time.sleep(4)
            run.cmd("<HIBERNATE> 1")
            time.sleep(4)
            after_hib = run.rss()
            out["run_b_image_evict"] = {
                "baseline_rss_mb": round(baseline_rss / 1024, 1),
                "heavy_loaded_rss_mb": round(loaded_rss / 1024, 1),
                "heavy_in_bg_rss_mb": round(with_heavy_bg / 1024, 1),
                "after_hibernate_rss_mb": round(after_hib / 1024, 1),
                "decoded_images_mb_estimate": 2 * 20,  # 2 tabs x 20 x 1MB
                "freed_by_hibernate_mb": round(
                    (with_heavy_bg - after_hib) / 1024, 1),
                "residual_vs_baseline_mb": round(
                    (after_hib - baseline_rss) / 1024, 1),
            }
            # ---- Run C: texture-cache residual quantified (3.5) ----
            # residual_vs_baseline after dropping BOTH heavy WebViews =
            # what the engine could not reclaim (GL/WR caches, allocator
            # retention beyond malloc_trim, service caches).
            out["run_c_texture_residual"] = {
                "note": "residual_vs_baseline_mb is the engine-side "
                        "memory not returned on full WebView drop; "
                        "Phase 3 attributed the bulk to the global "
                        "WebRender/texture caches",
                "residual_mb": out["run_b_image_evict"][
                    "residual_vs_baseline_mb"],
            }
        finally:
            run.close()

    srv.terminate()
    xvfb.terminate()
    pathlib.Path(args.out).parent.mkdir(parents=True, exist_ok=True)
    with open(args.out, "w") as f:
        json.dump(out, f, indent=2)
    print(json.dumps(out, indent=2))


if __name__ == "__main__":
    main()
