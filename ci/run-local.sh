#!/usr/bin/env bash
# Reproduce the CI pipeline locally before pushing.
set -euo pipefail
cd "$(dirname "$0")/.."
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo doc --no-deps
echo "local CI OK"
