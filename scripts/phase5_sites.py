#!/usr/bin/env python3
"""Phase 5: 30+ real-world site verification — brows12 vs Chromium side-by-side.

Resumable: any site whose ref+ours captures already exist is skipped unless
--force. Run in batches (the container kills long background work):

  python3 scripts/phase5_sites.py rw_example rw_wikipedia ...
  python3 scripts/phase5_sites.py --status     # show what is captured

Outputs:
  validation/run/phase5/ref/<name>.png        Chromium ground truth
  validation/run/phase5/ours/<name>.png       brows12 (Servo) capture
  validation/run/phase5/<name>.json           brows12 metrics
  screenshots/v2-servo/phase5/<name>_compare.png  side-by-side
"""
import json
import os
import pathlib
import subprocess
import sys

from PIL import Image, ImageDraw

ROOT = pathlib.Path(__file__).resolve().parent.parent
REF = ROOT / "validation" / "run" / "phase5" / "ref"
OURS = ROOT / "validation" / "run" / "phase5" / "ours"
COMPARE = ROOT / "screenshots" / "v2-servo" / "phase5"
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
    # controls
    ("rw_example", "https://example.com/"),
    ("rw_example_org", "https://example.org/"),
    ("rw_iana", "https://www.iana.org/"),
    # encyclopedia / docs
    ("rw_wikipedia", "https://www.wikipedia.org/"),
    ("rw_wiki_rust", "https://en.wikipedia.org/wiki/Rust_(programming_language)"),
    ("rw_docs_rs", "https://docs.rs/serde/latest/serde/"),
    ("rw_mdn", "https://developer.mozilla.org/en-US/docs/Web/CSS"),
    ("rw_web_dev", "https://web.dev/"),
    # developer sites
    ("rw_github", "https://github.com/"),
    ("rw_rust_lang", "https://www.rust-lang.org/"),
    ("rw_rust_blog", "https://blog.rust-lang.org/"),
    ("rw_crates_io", "https://crates.io/"),
    ("rw_python", "https://www.python.org/"),
    ("rw_go_dev", "https://go.dev/"),
    ("rw_kernel", "https://kernel.org/"),
    ("rw_gnu", "https://www.gnu.org/"),
    ("rw_debian", "https://www.debian.org/"),
    ("rw_arch", "https://archlinux.org/"),
    # news / text-first
    ("rw_hacker_news", "https://news.ycombinator.com/"),
    ("rw_sqlite", "https://sqlite.org/"),
    ("rw_npr_text", "https://text.npr.org/"),
    ("rw_cnn_lite", "https://lite.cnn.com/"),
    ("rw_habr", "https://habr.com/en/"),
    # history / standards
    ("rw_cern", "http://info.cern.ch"),
    ("rw_w3c", "https://www.w3.org/"),
    ("rw_whatwg", "https://html.spec.whatwg.org/"),
    ("rw_http3_is", "https://http3.is/"),
    # mixed complexity
    ("rw_caniuse", "https://caniuse.com/"),
    ("rw_arxiv", "https://arxiv.org/"),
    ("rw_mozilla", "https://www.mozilla.org/"),
    ("rw_py_docs", "https://docs.python.org/3/"),
    ("rw_v8_blog", "https://v8.dev/"),
]


def status():
    done = []
    for name, url in SITES:
        ok = ((OURS / f"{name}.png").exists() and (REF / f"{name}.png").exists())
        done.append((name, "OK" if ok else "todo", url))
    for _, s, u in done:
        print(f"{s:4} {u}")
    n_ok = sum(1 for _, s, _ in done if s == "OK")
    print(f"\n{n_ok}/{len(SITES)} captured")


def capture(names):
    for d in (REF, OURS, COMPARE):
        d.mkdir(parents=True, exist_ok=True)
    binary = ROOT / "target" / "debug" / "brows-servo"
    sites = [s for s in SITES if not names or s[0] in names]

    from playwright.sync_api import sync_playwright
    with sync_playwright() as pw:
        browser = pw.chromium.launch(args=["--force-device-scale-factor=1"])
        page = browser.new_page(viewport={"width": W, "height": H})
        for name, url in sites:
            if ((OURS / f"{name}.png").exists() and (REF / f"{name}.png").exists()):
                print(f"[{name}] cached, skip")
                continue
            print(f"[{name}] {url}", flush=True)

            print("  chrome…", end="", flush=True)
            try:
                page.goto(url, wait_until="domcontentloaded", timeout=45000)
                try:
                    page.wait_for_load_state("networkidle", timeout=8000)
                except Exception:
                    pass
                page.wait_for_timeout(1000)
                page.screenshot(path=str(REF / f"{name}.png"))
                print(" ok")
            except Exception as e:
                print(f" ERR {str(e)[:120]}")
                continue

            print("  brows12…", end="", flush=True)
            try:
                r = subprocess.run(
                    [str(binary), "--url", url, "--width", str(W), "--height", str(H),
                     "--png", str(OURS / f"{name}.png"),
                     "--json", str(ROOT / "validation" / "run" / "phase5" / f"{name}.json"),
                     "--timeout-ms", "45000", "--settle-ms", "3000"],
                    capture_output=True, text=True, timeout=110, env=SERVO_ENV)
                ok = r.returncode == 0 and (OURS / f"{name}.png").exists()
                print(" ok" if ok else f" FAILED rc={r.returncode} {r.stderr[-140:]}")
            except Exception as e:
                print(f" ERR {str(e)[:120]}")
        browser.close()
    print("batch done")


def compose_all():
    results = []
    for name, url in SITES:
        ref, ours = REF / f"{name}.png", OURS / f"{name}.png"
        entry = {"site": url, "name": name}
        if (ROOT / "validation" / "run" / "phase5" / f"{name}.json").exists():
            j = json.loads((ROOT / "validation" / "run" / "phase5" / f"{name}.json").read_text())
            entry["load_ms"] = j.get("load_complete_ms")
            entry["complete"] = j.get("complete")
            entry["crashed"] = j.get("crashed")
            entry["title"] = j.get("title")
        entry["captured"] = ref.exists() and ours.exists()
        if entry["captured"]:
            a, b = Image.open(ref).convert("RGB"), Image.open(ours).convert("RGB")
            canvas = Image.new("RGB", (a.width + b.width + 12,
                                       max(a.height, b.height) + LABEL_H), (24, 26, 27))
            d = ImageDraw.Draw(canvas)
            d.text((8, 7),
                   f"{name} — LEFT: Chromium | RIGHT: brows12 v2 (Servo)",
                   fill=(255, 255, 255))
            canvas.paste(a, (0, LABEL_H))
            canvas.paste(b, (a.width + 12, LABEL_H))
            out = COMPARE / f"{name}_compare.png"
            canvas.save(out)
            entry["compare"] = str(out.relative_to(ROOT))
        results.append(entry)
    (ROOT / "validation" / "run" / "phase5" / "summary.json").write_text(
        json.dumps(results, indent=2))
    ok = sum(1 for r in results if r["captured"])
    print(f"composed {ok}/{len(SITES)}; summary.json written")


if __name__ == "__main__":
    args = sys.argv[1:]
    if args and args[0] == "--status":
        status()
    elif args and args[0] == "--compose":
        compose_all()
    else:
        capture(args)
        compose_all()
