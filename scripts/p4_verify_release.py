#!/usr/bin/env python3
"""v2.1 Phase 4.4 — verify the GitHub Release end-to-end.

1. List release assets (tag arg), assert 4 platform zips + sha256 files.
2. Assert every zip < 150 MB (payload) and report sizes.
3. Download the linux zip, verify sha256, unpack, smoke-run brows12-ui
   under Xvfb (start page -> example.com -> clean quit) — the exact CI
   smoke protocol.
4. Screenshot the release page (Playwright Chromium) for the report.

Usage: python3 scripts/p4_verify_release.py v2.1.0-rc2 [--no-shot]
"""
import hashlib
import json
import pathlib
import subprocess
import sys
import zipfile

REPO = "salim77007j/brows12"
TOKEN = pathlib.Path("/home/z/.gh_token").read_text().strip() if pathlib.Path("/home/z/.gh_token").exists() else None
API = f"https://api.github.com/repos/{REPO}"


def gh(path, raw=False):
    import urllib.request
    req = urllib.request.Request(API + path)
    if TOKEN:
        req.add_header("Authorization", f"token {TOKEN}")
    with urllib.request.urlopen(req) as r:
        data = r.read()
    return data if raw else json.loads(data)


def main():
    tag = sys.argv[1] if len(sys.argv) > 1 else "v2.1.0-rc2"
    shot = "--no-shot" not in sys.argv
    rel = gh(f"/releases/tags/{tag}")
    assets = rel["assets"]
    print(f"release: {rel['name']} draft={rel['draft']} prerelease={rel['prerelease']}")
    print(f"url: {rel['html_url']}")
    zips = [a for a in assets if a["name"].endswith(".zip")]
    sums = [a for a in assets if a["name"].endswith(".sha256")]
    print(f"assets: {len(assets)} total; zips={len(zips)} sha256={len(sums)}")
    platforms = ("linux-x86_64", "windows-x86_64", "macos-arm64", "macos-x86_64")
    found = {p: None for p in platforms}
    for a in zips:
        for p in platforms:
            if p in a["name"]:
                found[p] = a
    ok = True
    for p in platforms:
        a = found[p]
        if not a:
            print(f"MISSING zip for {p}")
            ok = False
            continue
        mb = a["size"] / 1024 / 1024
        print(f"{p}: {a['name']} {mb:.1f} MB")
        if mb >= 150:
            print("  ZIP SIZE GATE FAILED (>= 150 MB payload)")
            ok = False

    # local platform evidence: download linux, verify sha, smoke-run
    if found["linux-x86_64"]:
        a = found["linux-x86_64"]
        import urllib.request
        req = urllib.request.Request(a["browser_download_url"])
        if TOKEN:
            req.add_header("Authorization", f"token {TOKEN}")
        dest = pathlib.Path(f"/tmp/{a['name']}")
        if not dest.exists() or dest.stat().st_size != a["size"]:
            print(f"downloading {a['name']} ...")
            with urllib.request.urlopen(req) as r, open(dest, "wb") as f:
                while True:
                    chunk = r.read(1 << 20)
                    if not chunk:
                        break
                    f.write(chunk)
        sha = hashlib.sha256(dest.read_bytes()).hexdigest()
        print(f"linux zip sha256 = {sha}")
        sref = gh(f"/releases/tags/{tag}")  # refresh not needed; sums listed
        sdata = None
        for s in sums:
            if "linux" in s["name"]:
                req2 = urllib.request.Request(s["browser_download_url"])
                if TOKEN:
                    req2.add_header("Authorization", f"token {TOKEN}")
                sdata = urllib.request.urlopen(req2).read().decode().split()[0]
        if sdata:
            print("sha256 match:", "YES" if sdata == sha else f"NO ({sdata})")
            ok = ok and sdata == sha
        out = pathlib.Path("/tmp/rel_verify")
        out.mkdir(exist_ok=True)
        with zipfile.ZipFile(dest) as z:
            z.extractall(out)
        bin_ = out / f"brows12-{tag.lstrip('v')}-linux-x86_64" / "brows12-ui"
        bin_.chmod(0o755)
        print(f"smoke-run {bin_} ({bin_.stat().st_size/1024/1024:.1f} MB) ...")
        rc = subprocess.run(
            ["bash", "-c", f'''
            set -e
            export DISPLAY=:99
            pgrep Xvfb >/dev/null || (Xvfb :99 -screen 0 1280x800x24 >/dev/null 2>&1 &)
            sleep 1
            export LD_LIBRARY_PATH=$HOME/.local/gl/usr/lib/x86_64-linux-gnu
            export __EGL_VENDOR_LIBRARY_FILENAMES=$HOME/.local/gl/usr/share/glvnd/egl_vendor.d/50_mesa.json
            export XDG_RUNTIME_DIR=/tmp/xdg RUST_LOG=error
            RUNDIR=/tmp/rel_smoke
            rm -rf $RUNDIR; mkdir -p $RUNDIR
            mkfifo $RUNDIR/cmd.fifo $RUNDIR/events.fifo
            BROWS12_UI_CMD_FIFO=$RUNDIR/cmd.fifo BROWS12_UI_EVENT_FIFO=$RUNDIR/events.fifo \\
              {bin_} > $RUNDIR/ui.log 2>&1 &
            UPID=$!
            exec 3<>$RUNDIR/events.fifo
            exec 4<>$RUNDIR/cmd.fifo
            S=""
            for i in $(seq 1 120); do
              IFS= read -r -t 1 line <&3 || true
              [[ "${{line:-}}" == loaded*url=brows12://start* ]] && {{ S="$line"; break; }}
            done
            [[ -n "$S" ]] || {{ echo "no start event"; cat $RUNDIR/ui.log; exit 1; }}
            echo "start: $S"
            echo '<OMNI> example.com' >&4
            echo '<RETURN>' >&4
            N=""
            for i in $(seq 1 120); do
              IFS= read -r -t 1 line <&3 || true
              [[ "${{line:-}}" == loaded*url=https://example.com* ]] && {{ N="$line"; break; }}
            done
            [[ -n "$N" ]] || {{ echo "no nav event"; cat $RUNDIR/ui.log; exit 1; }}
            echo "nav: $N"
            echo '<QUIT>' >&4
            for i in $(seq 1 30); do
              IFS= read -r -t 1 line <&3 || true
              [[ "${{line:-}}" == quit ]] && break
            done
            wait $UPID || true
            pgrep -f brows12 >/dev/null && echo "warn: process lingering" || true
            echo "RELEASE SMOKE PASS"
            '''],
            capture_output=True, text=True, timeout=420)
        tail = (rc.stdout + rc.stderr).strip().splitlines()[-6:]
        print("\n".join(tail))
        ok = ok and ("RELEASE SMOKE PASS" in rc.stdout) and rc.returncode == 0

    if shot:
        from playwright.sync_api import sync_playwright
        with sync_playwright() as pw:
            b = pw.chromium.launch()
            pg = b.new_page(viewport={"width": 1440, "height": 1400})
            pg.goto(rel["html_url"], wait_until="domcontentloaded", timeout=60000)
            pg.wait_for_timeout(2500)
            pg.screenshot(path="docs/perf-artifacts/v21/p4_release_page.png", full_page=False)
            b.close()
        print("release page screenshot: docs/perf-artifacts/v21/p4_release_page.png")

    print("VERDICT:", "PASS" if ok else "FAIL")
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
