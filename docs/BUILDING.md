# Building Brows12

## Prerequisites

- Rust stable (1.80+; developed on 1.99). `rustup` recommended.
- A C compiler for QuickJS-ng and ring (gcc/clang on Linux/macOS, MSVC on
  Windows). No cmake, no OpenSSL, no system libraries.

## Commands

```sh
# Everything (12 crates)
cargo build --workspace

# Tests: unit + integration across all crates
cargo test --workspace

# Lint (CI gate)
cargo clippy --workspace --all-targets -- -D warnings

# Benchmarks (compile) / run
cargo bench -p brows12-benchmarks --no-run
cargo bench -p brows12-benchmarks

# Engine comparison numbers
cargo run -q -p brows12-benchmarks --bin brows12-compare

# Docs
cargo doc --no-deps --open
```

## Feature flags

| Feature | Crate | Default | Effect |
|---|---|---|---|
| `http3` | `brows12-net` | off | HTTP/3 over QUIC (quinn + h3). Adds alt-svc-ready `h3_get`. |
| `doh` | `brows12-net` | on | RFC 8484 DNS-over-HTTPS client + CNAME chain extraction |
| `persist` | `brows12-storage` | on | redb-backed durable KV (off = pure memory profile) |
| `capi` | `brows12-api` | off | C ABI static library (`libbrows12_api.a` + `brows12.h`) |

## Platform notes

- **Linux** — primary target; x86_64 and aarch64 covered by bundled bindings.
- **Windows** — MSVC toolchain; ring ships pregenerated assembly; no POSIX
  quirks in the tree (paths via `std::path`, servers via `TcpListener`).
- **macOS** — works out of the box; not covered by CI yet (tracked).

## Fuzzing (nightly)

```sh
cd fuzz
cargo +nightly fuzz run html_parse -- -max_len=65536
cargo +nightly fuzz run css_parse
cargo +nightly fuzz run doh_wire_parse
cargo +nightly fuzz run cookie_parse
```

## Local CI

`./ci/run-local.sh` runs fmt-check, clippy `-D warnings`, the full test
suite and docs — the same gates as the GitHub workflow.
