#!/usr/bin/env python3
"""Phase 4 Area 2.7 E2E verification: response-header security guard.

Loopback fixture (three distinct loopback hosts = three "sites"):
  127.0.0.1:MAIN  main page, response carries
                  Content-Security-Policy: script-src 'self'
                  Strict-Transport-Security: max-age=31536000
                  page loads evil.js from 127.0.0.2 and embeds an
                  <iframe src=127.0.0.3/frame>
  127.0.0.2:EVIL  serves evil.js (cross-site under the page's CSP)
  127.0.0.3:FRAME serves the frame target with X-Frame-Options: DENY

Expected (Chrome parity):
  * evil.js is blocked (brows12: embedder CSP guard; Chrome: CSP engine)
  * the frame is blocked (brows12: XFO probe guard; Chrome: XFO engine)
  * HSTS learned at runtime (brows12 counter; Chrome has no counter)
Run:
  python3 scripts/phase4_area2_7.py          # full check + side-by-side
  python3 scripts/phase4_area2_7.py --no-chrome
"""
import json
import os
import subprocess
import sys
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
RUN = ROOT / "validation" / "run" / "area2_7"
RUN.mkdir(parents=True, exist_ok=True)

MAIN_PORT, EVIL_PORT, FRAME_PORT = 8951, 8952, 8953

SERVO_ENV = {
    **os.environ,
    "DISPLAY": ":99",
    "LD_LIBRARY_PATH": os.path.expanduser("~/.local/gl/usr/lib/x86_64-linux-gnu"),
    "__EGL_VENDOR_LIBRARY_FILENAMES": os.path.expanduser(
        "~/.local/gl/usr/share/glvnd/egl_vendor.d/50_mesa.json"),
    "XDG_RUNTIME_DIR": "/tmp/xdg",
    "RUST_LOG": "error",
}

MAIN_HTML = f"""<!DOCTYPE html>
<html><head><meta charset="utf-8"><title>Area 2.7 security guard</title>
<style>
body {{ font-family: sans-serif; background: #0f1320; color: #e8eaf2; padding: 24px; }}
h1 {{ font-size: 22px; }}
.box {{ padding: 14px 18px; border-radius: 10px; margin: 12px 0; font-size: 17px; }}
#csp {{ background: #12351f; border: 2px solid #2e9e57; }}
#frame {{ background: #331a1a; border: 2px solid #a04b4b; height: 120px; }}
iframe {{ width: 100%; height: 110px; border: 0; }}
</style></head>
<body>
<h1>Phase 4 · Area 2.7 — CSP + X-Frame-Options enforcement</h1>
<div class="box" id="csp">CSP check pending…</div>
<script src="http://127.0.0.2:{EVIL_PORT}/evil.js"></script>
<div class="box" id="frame">
  <div>Embedding http://127.0.0.3:{FRAME_PORT}/frame (X-Frame-Options: DENY):</div>
  <iframe src="http://127.0.0.3:{FRAME_PORT}/frame"></iframe>
</div>
</body></html>"""

FRAME_HTML = """<!DOCTYPE html>
<html><body style="background:#7a1f1f;color:#fff;font:20px sans-serif;padding:16px">
FRAME CONTENT VISIBLE — XFO BYPASSED (BAD)</body></html>"""

EVIL_JS = f"""
document.getElementById('csp').textContent =
  'CSP VIOLATED: cross-site script executed (BAD)';
document.title = 'CSP VIOLATED';
"""


class MainServer(BaseHTTPRequestHandler):
    def do_GET(self):
        body = MAIN_HTML.encode()
        self.send_response(200)
        self.send_header("Content-Type", "text/html; charset=utf-8")
        self.send_header("Content-Security-Policy", "script-src 'self'")
        self.send_header("Strict-Transport-Security", "max-age=31536000")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *a):
        pass


class EvilServer(BaseHTTPRequestHandler):
    def do_GET(self):
        body = EVIL_JS.encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/javascript")
        self.send_header("Access-Control-Allow-Origin", "*")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *a):
        pass


class FrameServer(BaseHTTPRequestHandler):
    def do_GET(self):
        body = FRAME_HTML.encode()
        self.send_response(200)
        self.send_header("Content-Type", "text/html; charset=utf-8")
        self.send_header("X-Frame-Options", "DENY")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *a):
        pass


def serve(handler, ip, port):
    srv = ThreadingHTTPServer((ip, port), handler)
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    return srv


def run_brows12():
    out = RUN / "brows12.json"
    png = RUN / "brows12.png"
    subprocess.run(
        [
            str(ROOT / "target" / "debug" / "brows-servo"),
            "--url", f"http://127.0.0.1:{MAIN_PORT}/",
            "--json", str(out), "--png", str(png),
            "--timeout-ms", "60000",
        ],
        cwd=ROOT, check=True, timeout=120, env=SERVO_ENV,
        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
    )
    return json.loads(out.read_text()), png


def run_chrome():
    png = RUN / "chrome.png"
    from playwright.sync_api import sync_playwright

    with sync_playwright() as p:
        browser = p.chromium.launch()
        page = browser.new_page(viewport={"width": 1280, "height": 800})
        page.goto(f"http://127.0.0.1:{MAIN_PORT}/", wait_until="networkidle",
                  timeout=30000)
        page.wait_for_timeout(500)
        csp_text = page.text_content("#csp")
        page.screenshot(path=str(png))
        browser.close()
        return csp_text, png


def side_by_side(a: Path, b: Path, out: Path):
    from PIL import Image, ImageDraw

    ia, ib = Image.open(a).convert("RGB"), Image.open(b).convert("RGB")
    w, h, label = 1280, 800, 28
    canvas = Image.new("RGB", (w * 2 + 12, h + label), (24, 24, 28))
    canvas.paste(ia, (0, label))
    canvas.paste(ib, (w + 12, label))
    d = ImageDraw.Draw(canvas)
    d.text((8, 7), "Chrome 126 (ground truth)", fill=(255, 255, 255))
    d.text((w + 20, 7), "brows12 (Servo 0.6 + 2.7 guard)", fill=(120, 220, 140))
    out.parent.mkdir(parents=True, exist_ok=True)
    canvas.save(out)


def main():
    no_chrome = "--no-chrome" in sys.argv
    servers = [
        serve(MainServer, "127.0.0.1", MAIN_PORT),
        serve(EvilServer, "127.0.0.2", EVIL_PORT),
        serve(FrameServer, "127.0.0.3", FRAME_PORT),
    ]
    try:
        report, brows_png = run_brows12()
        priv = report["privacy"]
        checks = {
            "page loaded (LoadStatus::Complete)": report["complete"],
            "no crash": not report["crashed"],
            "security_probes >= 2 (main doc + frame target)":
                priv["security_probes"] >= 2,
            "csp_blocked >= 1 (evil.js denied)":
                priv["csp_blocked"] >= 1,
            "frames_blocked >= 1 (XFO frame denied)":
                priv["frames_blocked"] >= 1,
            "hsts_learned >= 1 (runtime HSTS from header)":
                priv["hsts_learned"] >= 1,
            "evil.js not executed (page title intact)":
                report.get("title") != "CSP VIOLATED",
        }
        print("brows12 counters:", json.dumps(
            {k: priv[k] for k in
             ("security_probes", "csp_blocked", "frames_blocked",
              "mixed_content_blocked", "hsts_learned", "coop_observed",
              "coep_observed", "corp_observed")}))
        print("frame_log:", priv["frame_log"])
        print("csp_block_log:", priv["csp_block_log"])

        chrome_csp_text = chrome_png = None
        if not no_chrome:
            chrome_csp_text, chrome_png = run_chrome()
            print(f"Chrome #csp text: {chrome_csp_text!r}")
            # Chrome enforces the CSP itself: the script never runs.
            checks["chrome blocks evil.js too (parity)"] = "VIOLATED" not in (
                chrome_csp_text or "")
            side_by_side(
                chrome_png, brows_png,
                ROOT / "screenshots" / "v2-servo" / "phase4"
                / "area2_7_security_side_by_side.png")

        failed = [name for name, ok in checks.items() if not ok]
        for name, ok in checks.items():
            print(("PASS " if ok else "FAIL ") + name)
        if failed:
            sys.exit(1)
        print("ALL AREA 2.7 CHECKS PASS")
    finally:
        for s in servers:
            s.shutdown()


if __name__ == "__main__":
    main()
