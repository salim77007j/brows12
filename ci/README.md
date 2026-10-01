# CI

GitHub Actions workflows live in `.github/workflows/` (GitHub only reads
them from there):

- `ci.yml` — build, test, clippy `-D warnings`, bench smoke on Linux + Windows.
- `fuzz.yml` — nightly fuzz smoke build of the `fuzz/` harnesses.

`run-local.sh` reproduces the same gates locally.
