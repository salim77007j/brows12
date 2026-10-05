#!/usr/bin/env python3
"""v2.1 Phase 1.1 — Chrome floor calibration: example.com + cnn peaks
via the same tree-sampling protocol as area3_compare.measure_chrome."""
import json
import pathlib
import sys

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent.parent / "scripts"))
from area3_compare import measure_chrome  # noqa: E402

ROOT = pathlib.Path(__file__).resolve().parent.parent
out = {}

for name, url in [("example", "https://example.com/"),
                  ("hackernews", "https://news.ycombinator.com/"),
                  ("cnn", "https://www.cnn.com/")]:
    r = measure_chrome(url, settle_ms=6000, timeout_ms=90000)
    if r is None:
        print(f"[fail] chrome {name}")
        continue
    out[name] = r
    print(f"[ok] chrome {name}: peak {r['peak_kb']/1024:.1f} MB "
          f"(rss {r['rss_kb']/1024:.1f}, pss {r['pss_kb']/1024:.1f})")
    (ROOT / "docs/perf-artifacts/v21/p11_chrome_floor.json").write_text(
        json.dumps(out, indent=1))
