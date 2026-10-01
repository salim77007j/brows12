#!/usr/bin/env bash
# Cross-browser comparison: measures cold start + page load for Chrome,
# Firefox, Brave (if installed) and Brows12 using hyperfine.
# Requires: hyperfine (https://github.com/sharkdp/hyperfine), a browser.
set -euo pipefail

cd "$(dirname "$0")/../../.."
OUT="${1:-bench-results}"
mkdir -p "$OUT"

# 1. Engine numbers.
cargo build --release -p brows12-benchmarks --bin brows12-compare
./target/release/brows12-compare > "$OUT/brows12.txt"

# 2. Browser cold start (about:blank), if the binary exists.
measure_browser() {
  local name="$1"; shift
  if ! command -v "$name" >/dev/null 2>&1; then
    echo "skip: $name not installed" >&2
    return
  fi
  hyperfine --warmup 2 --min-runs 10 \
    --export-json "$OUT/${name}.json" \
    "$name $*"
}

measure_browser google-chrome --headless --disable-gpu --dump-dom about:blank
measure_browser chromium --headless --disable-gpu --dump-dom about:blank
measure_browser firefox --headless --dump-dom about:blank
measure_browser brave-browser --headless --disable-gpu --dump-dom about:blank

echo "results in $OUT/"
