#!/usr/bin/env python3
"""v2.1 Phase 1.1 — A/B for the display-bound decode cap on cnn.com.

Runs the same measurement twice on the CURRENT binary:
  cap-on   : defaults (2560x1600)
  cap-off  : BROWS12_IMAGE_DECODE_MAX_W/H=0 (upstream behavior)
Prints peak deltas and writes JSON for the report.
"""
import json
import os
import pathlib
import sys

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import p11_cnn_diag as m  # noqa: E402

ROOT = pathlib.Path(__file__).resolve().parent.parent


def run(tag, env_extra):
    os.environ.pop("BROWS12_IMAGE_DECODE_MAX_W", None)
    os.environ.pop("BROWS12_IMAGE_DECODE_MAX_H", None)
    os.environ.pop("BROWS12_DIAG_DENY", None)
    m.GL_ENV.pop("BROWS12_IMAGE_DECODE_MAX_W", None)
    m.GL_ENV.pop("BROWS12_IMAGE_DECODE_MAX_H", None)
    m.GL_ENV.pop("BROWS12_DIAG_DENY", None)
    m.GL_ENV.update(env_extra)
    os.environ.update(env_extra)
    r = m.measure(settle_ms=6000, timeout_ms=90000)
    if r is None:
        print(f"[fail] {tag}")
        sys.exit(1)
    peak = r["peak_kb"] / 1024
    print(f"[ok] {tag}: peak {peak:.1f} MB")
    return r


def main():
    out = ROOT / "docs/perf-artifacts/v21/p11_cnn_cap_ab.json"
    off = run("cap-off", {"BROWS12_IMAGE_DECODE_MAX_W": "0",
                          "BROWS12_IMAGE_DECODE_MAX_H": "0"})
    on = run("cap-on", {})
    data = {
        "url": m.URL,
        "cap_off_peak_kb": off["peak_kb"],
        "cap_on_peak_kb": on["peak_kb"],
        "saved_mb": round((off["peak_kb"] - on["peak_kb"]) / 1024, 1),
        "load_ms": {
            "cap_off": off["perf"]["startup"]["first_load_complete_ms"],
            "cap_on": on["perf"]["startup"]["first_load_complete_ms"],
        },
    }
    out.write_text(json.dumps(data, indent=1))
    print(json.dumps(data, indent=1))


if __name__ == "__main__":
    main()
