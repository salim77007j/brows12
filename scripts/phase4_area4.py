#!/usr/bin/env python3
"""Phase 4 Area 4 E2E verification — tab innovation (shell-level).

4.1 Predictive hibernation: 4 tabs with a deterministic activation
pattern (tab2 x3, tab1 x1 recent, tab3 never re-activated) under a
tight governor budget. The event stream must show:
  * `predict_order` events (the predictive ranking runs),
  * the FIRST hibernated tab == the FIRST predicted candidate,
  * tab3 (zero activations, stale) hibernates before tab0/tab2
    (frequently activated).

Chrome parity note: tab-management is shell behavior; headless Chromium
exposes no tab-strip/governor observable, so verification is by
event-stream invariants + unit tests (documented in the Area 4 report).

Usage: python3 scripts/phase4_area4.py --only 4.1 --out docs/perf-artifacts/phase4/area4/area4_41.json
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


class UiRun:
    """One brows12-ui automation session under Xvfb (Area 3 harness)."""

    def __init__(self, env_extra, name):
        self.name = name
        self.dir = pathlib.Path(f"/tmp/a4-{name}")
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
        raise TimeoutError(f"{what} ({self.name}): tail={self.events[-5:]}")

    def close(self):
        try:
            self.cmd("<QUIT>")
            self.proc.wait(timeout=15)
        except Exception:
            self.proc.kill()


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


def open_tab(run, url, marker, settle=1.0):
    """One new tab navigating to url; waits for its `loaded` event."""
    run.cmd("<NEWTAB>")
    time.sleep(0.4)
    run.cmd(f"<OMNI> {url}")
    time.sleep(0.2)
    run.cmd("<RETURN>")
    end = time.time() + 120
    while time.time() < end:
        n = sum(1 for e in run.events
                if e.startswith("loaded tab=") and marker in e)
        if n >= 1:
            break
        time.sleep(0.25)
    else:
        raise TimeoutError(f"load {url} ({run.name}): {run.events[-5:]}")
    time.sleep(settle)


def parse_predict_order(e):
    """`predict_order why=rss level=Critical candidates=[3, 0, 2]`"""
    if "candidates=[" not in e:
        return None
    body = e.split("candidates=")[1].strip()
    body = body.strip("[]").replace(" ", "")
    return [int(x) for x in body.split(",") if x] if body else []


def hibernated_indices(events):
    """Indices from `hibernate tab=<id> index=<i> ...` events."""
    out = []
    for e in events:
        if e.startswith("hibernate tab="):
            for tok in e.split():
                if tok.startswith("index="):
                    out.append(int(tok.split("=")[1]))
    return out


# ---------------------------------------------------------------- 4.1 --
def verify_41(out):
    site = pathlib.Path("/tmp/a4_site")
    site.mkdir(parents=True, exist_ok=True)
    for name, title in (("a.html", "A4 page a"),
                        ("b.html", "A4 page b"),
                        ("c.html", "A4 page c")):
        (site / name).write_text(
            f"<!doctype html><html><head><title>{title}</title></head>"
            f"<body><h1>{title}</h1><p>predictive hibernation fixture.</p>"
            "</body></html>")
    srv = subprocess.Popen(
        [sys.executable, "-m", "http.server", "8137", "--directory", str(site)],
        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    assert wait_http("http://127.0.0.1:8137/a.html"), "http server"
    try:
        env = {
            # Tiny budget: debug-build RSS crosses Critical immediately,
            # so the governor's predictive reclaim runs every tick.
            "BROWS12_MEM_BUDGET_MB": "64",
            "BROWS12_GOVERNOR_INTERVAL_MS": "1500",
            "BROWS12_GOVERNOR_WARMUP_MS": "26000",
            "BROWS12_TAB_SUSPEND_SECS": "3",
            "BROWS12_HEAVY_SUSPEND_SECS": "2",
        }
        run = UiRun(env, "41")
        try:
            run.wait_for(lambda e: e.startswith("start "), 60, "start")
            # Tabs: 0 = start page, 1 = a.html, 2 = b.html, 3 = c.html
            open_tab(run, "http://127.0.0.1:8137/a.html", "a.html")
            open_tab(run, "http://127.0.0.1:8137/b.html", "b.html")
            open_tab(run, "http://127.0.0.1:8137/c.html", "c.html")
            # Activation pattern: tab2 x3 (alternating through tab0),
            # tab1 once (it ends ACTIVE), tab3 never re-activated.
            for _ in range(3):
                run.cmd("<SWITCH> 2")
                time.sleep(0.35)
                run.cmd("<SWITCH> 0")
                time.sleep(0.35)
            run.cmd("<SWITCH> 1")
            # Governor ticks: predictive reclaim fires; every background
            # tab is >3 s idle well within 20 s.
            time.sleep(25)
            events = list(run.events)
            orders = [parse_predict_order(e) for e in events
                      if e.startswith("predict_order")]
            orders = [o for o in orders if o]
            hibernated = hibernated_indices(events)
            first_hib = hibernated[0] if hibernated else None
            checks = {
                "predict_order events emitted": len(orders) > 0,
                "first prediction = tab3 (zero activations)":
                    bool(orders) and orders[0] and orders[0][0] == 3,
                "first hibernated tab == first predicted candidate":
                    first_hib is not None and bool(orders)
                    and first_hib == orders[0][0],
                "tab3 hibernated before tab2 (frequency ordering)":
                    hibernated and 3 in hibernated and 2 in hibernated
                    and hibernated.index(3) < hibernated.index(2),
                "governor reclaimed under pressure": len(hibernated) >= 2,
            }
            out["area4_1"] = {
                "predictions": orders[:6],
                "hibernated_order": hibernated,
                "checks": checks,
            }
            failed = [k for k, ok in checks.items() if not ok]
            for k, ok in checks.items():
                print(("PASS " if ok else "FAIL ") + k)
            print("predictions:", orders[:6])
            print("hibernated:", hibernated)
            return not failed
        finally:
            run.close()
    finally:
        srv.terminate()


# ---------------------------------------------------------------- 4.2 --
TALL_HTML = """<!doctype html><html><head><title>A4 discard fixture</title></head>
<body style="height:3000px">
<h1>discard preservation fixture</h1>
<input id="f1" placeholder="type here">
<script>
// Simulates user input a moment after load (automation cannot focus
// page widgets from the shell FIFO).
setTimeout(function () {
  document.getElementById('f1').value = 'preserved-text-42';
}, 1500);
</script>
</body></html>"""


def verify_42(out):
    site = pathlib.Path("/tmp/a4_site2")
    site.mkdir(parents=True, exist_ok=True)
    (site / "tall.html").write_text(TALL_HTML)
    srv = subprocess.Popen(
        [sys.executable, "-m", "http.server", "8138", "--directory", str(site)],
        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    assert wait_http("http://127.0.0.1:8138/tall.html"), "http server"
    try:
        env = {
            "BROWS12_MEM_BUDGET_MB": "64",
            "BROWS12_GOVERNOR_INTERVAL_MS": "1500",
            "BROWS12_GOVERNOR_WARMUP_MS": "18000",
            "BROWS12_TAB_SUSPEND_SECS": "3",
            "BROWS12_HEAVY_SUSPEND_SECS": "2",
        }
        run = UiRun(env, "42")
        try:
            run.wait_for(lambda e: e.startswith("start "), 60, "start")
            # tab1: tall page; wait for load + the simulated user input.
            open_tab(run, "http://127.0.0.1:8138/tall.html", "tall.html")
            time.sleep(2.5)
            # Scroll down 3x800 px (wheel-forward; estimate tracked).
            for _ in range(3):
                run.cmd("<SCROLL> 800")
                time.sleep(0.3)
            # Leave the tab (form snapshot fires on switch-away), let the
            # governor discard it, then come back.
            run.cmd("<SWITCH> 0")
            time.sleep(1.0)
            discarded = run.wait_for(
                lambda e: e.startswith("tab_discarded index=1 "),
                60, "tab_discarded")
            hib = run.wait_for(
                lambda e: e.startswith("hibernate tab=") and "index=1" in e,
                30, "hibernate index=1")
            time.sleep(2.0)  # settle before restoring
            run.cmd("<SWITCH> 1")
            state_restore = run.wait_for(
                lambda e: e.startswith("state_restore index=1"),
                120, "state_restore index=1")
            checks = {
                "governor discarded the background tab": bool(discarded),
                "discard preserved the scroll estimate":
                    "scroll_est=2400" in discarded,
                "discard preserved a form snapshot": "forms=1" in discarded,
                "restore re-applied scroll+form state":
                    "scroll_est=2400" in state_restore and "forms=1" in state_restore,
                "hibernate followed the discard": bool(hib),
            }
            out["area4_2"] = {
                "tab_discarded": discarded,
                "state_restore": state_restore,
                "checks": checks,
            }
            for k, ok in checks.items():
                print(("PASS " if ok else "FAIL ") + k)
            print("discarded:", discarded)
            print("state_restore:", state_restore)
            return all(checks.values())
        finally:
            run.close()
    finally:
        srv.terminate()


# ---------------------------------------------------------------- 4.3 --
def verify_43(out):
    """Tab groups data layer: create, membership, JSON dump, collapse,
    delete-ungroups. Driven end-to-end through the FIFO; the strip
    renders the group color bar (visually verifiable in the artifact)."""
    run = UiRun({"BROWS12_GOVERNOR": "0"}, "43")
    try:
        run.wait_for(lambda e: e.startswith("start "), 60, "start")
        run.cmd("<NEWTAB>"); time.sleep(0.5)
        run.cmd("<NEWTAB>"); time.sleep(0.5)
        # 3 tabs: 0 start, 1 start, 2 start
        run.cmd("<GROUP_NEW> research|purple")
        created = run.wait_for(lambda e: e.startswith("group_created "),
                               15, "group_created")
        gid = int(created.split("id=")[1].split()[0])
        run.cmd(f"<GROUP_ADD> {gid}|0")
        run.wait_for(lambda e: e.startswith("group_add") and "ok=true" in e,
                     15, "group_add 0")
        run.cmd(f"<GROUP_ADD> {gid}|2")
        run.wait_for(lambda e: e.startswith(f"group_add group={gid} tab=2 ok=true"),
                     15, "group_add 2")
        run.cmd("<GROUPS>")
        dump = run.wait_for(lambda e: e.startswith("groups_json "), 15, "groups_json")
        # Space-free JSON payload (ev_escape turns spaces into _).
        js = dump.split("groups_json ", 1)[1].strip()
        data = json.loads(js)
        g = data["groups"][0]
        # Marker AFTER the first dump so the second-dump scan below only
        # sees events emitted from here on.
        mark = len(run.events)
        run.cmd(f"<GROUP_TOGGLE> {gid}")
        tog = run.wait_for(lambda e: e.startswith("group_toggle"), 15, "group_toggle")
        run.cmd(f"<GROUP_DEL> {gid}")
        dele = run.wait_for(lambda e: e.startswith("group_deleted"), 15, "group_deleted")
        run.cmd("<GROUPS>")
        # Only look at events AFTER the first dump (wait_for scans all).
        dump2 = None
        end = time.time() + 15
        while time.time() < end and dump2 is None:
            new = [e for e in run.events[mark:] if e.startswith("groups_json ")]
            if len(new) >= 1:
                dump2 = new[-1]
            time.sleep(0.1)
        assert dump2, "second groups_json"
        js2 = dump2.split("groups_json ", 1)[1].strip()
        data2 = json.loads(js2)
        checks = {
            "group created with name+color":
                g["name"] == "research" and g["color"] == "purple",
            "membership: strip tabs 0 and 2 (shell ids 1 and 3)":
                sorted(g["tabs"]) == [1, 3],
            "collapse toggles": "collapsed=1" in tog,
            "delete reports ok": "ok=true" in dele,
            "delete ungrouped members": data2["groups"] == [],
        }
        out["area4_3"] = {"group": g, "checks": checks}
        for k, ok in checks.items():
            print(("PASS " if ok else "FAIL ") + k)
        print("group:", g)
        return all(checks.values())
    finally:
        run.close()


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--only", default="all",
                    help="4.1 | all (more sub-items land per commit)")
    ap.add_argument("--out",
                    default="docs/perf-artifacts/phase4/area4/area4_verify.json")
    args = ap.parse_args()
    pathlib.Path(args.out).parent.mkdir(parents=True, exist_ok=True)
    out = {}
    if pathlib.Path(args.out).exists():
        try:
            out = json.loads(pathlib.Path(args.out).read_text())
        except Exception:
            out = {}
    ok = True
    if args.only in ("all", "4.1"):
        ok &= verify_41(out)
    if args.only in ("all", "4.2"):
        ok &= verify_42(out)
    if args.only in ("all", "4.3"):
        ok &= verify_43(out)
    pathlib.Path(args.out).write_text(json.dumps(out, indent=2))
    print(f"artifact: {args.out}")
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
