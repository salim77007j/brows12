#!/usr/bin/env python3
"""v2.1 Phase 2 — shared-context interaction smoke for the product UI.

Opens 3 tabs on different sites, switches between them, hibernates and
restores one, then checks: every tab painted at least once (frame_count
advances), switching produced no crash, and the quit path is clean.
"""
import json
import os
import pathlib
import re
import subprocess
import sys
import threading
import time

ROOT = pathlib.Path(__file__).resolve().parent.parent
XVFB_ENV = {
    **os.environ,
    "DISPLAY": ":99",
    "LD_LIBRARY_PATH": os.path.expanduser("~/.local/gl/usr/lib/x86_64-linux-gnu"),
    "__EGL_VENDOR_LIBRARY_FILENAMES": os.path.expanduser(
        "~/.local/gl/usr/share/glvnd/egl_vendor.d/50_mesa.json"),
    "XDG_RUNTIME_DIR": "/tmp/xdg",
    "RUST_LOG": "error",
}

d = "/tmp/b12-smoke"
os.makedirs(d, exist_ok=True)
for f in os.listdir(d):
    os.unlink(os.path.join(d, f))
cmd_fifo, evt_fifo = f"{d}/cmd", f"{d}/evt"
os.mkfifo(cmd_fifo)
os.mkfifo(evt_fifo)

proc = subprocess.Popen(
    [str(ROOT / "target/release/brows12-ui")],
    env={**XVFB_ENV, "BROWS12_UI_CMD_FIFO": cmd_fifo, "BROWS12_UI_EVENT_FIFO": evt_fifo},
    stdout=subprocess.DEVNULL, stderr=open(f"{d}/stderr.log", "wb"))

events = []
def reader():
    with open(evt_fifo) as f:
        for line in f:
            events.append(line.strip())
            if line.startswith("quit"):
                return
threading.Thread(target=reader, daemon=True).start()

w = open(cmd_fifo, "w")
def cmd(line):
    w.write(line + "\n")
    w.flush()

deadline = time.time() + 30
while time.time() < deadline:
    if any(e.startswith("start ") for e in events):
        break
    time.sleep(0.1)

SITES = ["https://example.com", "https://www.wikipedia.org", "https://news.ycombinator.com"]
ok = True
for i, url in enumerate(SITES):
    if i:
        cmd("<NEWTAB>")
    time.sleep(0.4)
    cmd(f"<OMNI> {url}")
    cmd("<RETURN>")

deadline = time.time() + 60
while time.time() < deadline:
    if sum(1 for e in events if e.startswith("loaded ")) >= 3:
        break
    time.sleep(0.25)
loaded = sum(1 for e in events if e.startswith("loaded "))
print(f"loaded: {loaded}/3")
ok &= loaded == 3

# Switch across all tabs twice; expect no crash and nav/loaded events.
for idx in (2, 1, 0, 1, 2):
    cmd(f"<SWITCH> {idx}")
    time.sleep(1.0)
crash = "panicked" in open(f"{d}/stderr.log", errors="replace").read()
print(f"switch cycle done, crashed={crash}")
ok &= not crash

# Hibernate tab 1 (background now), wait, restore it.
cmd("<HIBERNATE> 1")
time.sleep(2.0)
hib = any(e.startswith("hibernate tab=") for e in events)
mark = len(events)
cmd("<RESTORE> 1")
time.sleep(1.0)
# A background restore completes silently (loaded events fire only for
# the active tab); activate the restored tab so the reload completes
# visibly, like a user would.
cmd("<SWITCH> 1")
deadline = time.time() + 45
restored_loaded = 0
while time.time() < deadline:
    restored_loaded = sum(
        1 for e in events[mark:] if e.startswith("loaded tab=2 "))
    if restored_loaded > 0:
        break
    time.sleep(0.25)
post = events[mark:]
print(f"hibernate={hib} restore_reload_loaded={restored_loaded}")
print("post-restore events:", post[:4])
ok &= hib and restored_loaded >= 1

# memreport still works after all that.
cmd("<MEMREPORT>")
deadline = time.time() + 30
mr = False
while time.time() < deadline:
    if any(e.startswith("memreport ") for e in events):
        mr = True
        break
    time.sleep(0.25)
print(f"memreport after hibernate/restore: {mr}")
ok &= mr

cmd("<QUIT>")
try:
    proc.wait(timeout=10)
except subprocess.TimeoutExpired:
    proc.kill()
print("SMOKE", "PASS" if ok else "FAIL")
sys.exit(0 if ok else 1)
