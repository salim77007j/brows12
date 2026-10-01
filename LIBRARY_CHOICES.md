# Library Choices

The Brows12 philosophy: **assemble the 2026 winners, write custom code only
where no adequate library exists.** Every dependency below was selected by
checking the crates.io registry for the newest maintained version, reading
the resolved source, and weighing three axes: raw performance, memory
profile, and API/embeddability. Versions are the resolved majors at the time
of writing; `Cargo.lock` pins exact builds.

## Summary table

| Domain | Chosen | Rejected (and why) |
|---|---|---|
| Language | Rust 2021 (stable 1.99) | C++ (memory safety), Go (GC pauses, runtime) |
| JavaScript engine | **QuickJS-ng 2026** via `rquickjs` 0.14 | V8 (100+ MB RAM footprint, huge build), SpiderMonkey (same), Boa (10-40× slower on JS benchmarks, still maturing) |
| HTML parsing | **html5ever 0.39** + `markup5ever_rcdom` | lol_html (streaming rewriter, not a tree builder), swc_html (fast but parsing API churn, bigger dep tree), html-parser (unmaintained) |
| CSS parsing | **lightningcss 1.0.0-alpha.72** | stylo/servo (cannot be embedded without the whole Servo stack), grass (Sass semantics, not a cascade engine), cssparser alone (too low-level) |
| Selector matching | **Custom matcher over `parcel_selectors` 0.28 AST** | servo `selectors` crate direct embed (private Impl types inside lightningcss, version coupling, needs Stylo-style Element trait); see rationale below |
| Layout | **taffy 0.14** | morphorm (smaller feature set), stretch (dead), custom Cassowary solver (months of work) |
| Text shaping | **cosmic-text 0.19** (rustybuzz + swash) | rustybuzz alone (no layout runs/bidi wrapping), fontdue (rasterization only, no complex shaping), harfbuzz-sys (C dependency) |
| 2D raster | **tiny-skia 0.12** | vello (GPU-first; wonderful, but adds wgpu driver matrix to CI and ~20 s cold start; tracked as opt-in), raqote (slower than tiny-skia in resvg comparisons), skia-bindings (giant C++ build) |
| GPU compositing | tiny-skia now; **vello/wgpu planned** behind a feature | — |
| HTTP/1.1 + 2 | **hyper 1.x + hyper-util 0.1** | reqwest (full client but opinionated, hides the connection layer we need for cookies/DoH/alt-svc), ureq (blocking only) |
| TLS | **rustls 0.23 (ring provider)** | native-tls/OpenSSL (C, CVE surface, platform variance), aws-lc-rs (cmake/NASM build burden for marginal gain) |
| HTTP/3 | **quinn 0.11 + h3 0.0.8 + h3-quinn 0.0.10** (feature `http3`) | s2n-quic (AWS-centric, heavier), msquic (C) |
| DNS | **Hand-rolled RFC 8484 DoH client** over our own HTTPS stack | hickory-resolver (excellent, but pulls a second async resolver stack + config surface for one API call; our wire-format code is ~200 lines, fuzzed, and gives us CNAME chains for cloaking detection) |
| Async runtime | **tokio 1.x** (multi-thread) | smol (fine, smaller ecosystem), monoio/glommio (io_uring-only wins, Linux-only) |
| Image decode | **image 0.25** (png/jpeg/webp/gif backends) | zune-* family directly (image already delegates to zune-jpeg internally), libpng/jpeg-turbo-sys (C) |
| DOM | **Custom arena DOM** + html5ever tree builder | Rc-cycle DOMs (refcount churn, O(n) teardown), webrender-scene graphs (overkill) |
| Cookies | **Custom RFC 6265bis jar + CHIPS** | cookie crate (parses, but no jar semantics, no partitioning, no public-suffix enforcement) |
| Public suffix | **psl 2.1** | publicsuffix (unmaintained) |
| Storage engine | **redb 4** (feature `persist`) + in-memory default | sled (maintenance status), rocksdb (C++), sqlite (C, SQL overhead for KV) |
| Serialization | **serde 1 + serde_json 1** | JSON5/toml variants (not needed here) |
| Logging | **tracing 0.1 + tracing-subscriber** | log (no structured spans), slog (superseded) |
| Errors | **thiserror 2 (libs) + anyhow 1 (tools)** | manual Display impls, error-chain (dead) |
| Time | **jiff 0.2 + httpdate** | chrono (legacy API), time (fine, jiff is the modern successor) |
| Crypto/hash | **blake3 1** (cache keys), rustls/ring primitives | SHA-1/MD5 anything, ring directly for hashing (blake3 is faster and simpler) |
| Randomness | **fastrand 2** | rand (heavier than needed for noise injection) |
| URLs | **url 2.5** (Servo) | idna reimplementation (no) |
| Base64 | **base64 0.22** | hand-rolled |
| Benchmarks | **criterion 0.8** | divan (promising, but criterion's reports/tooling are the team standard) |
| Fuzzing | **cargo-fuzz + libfuzzer-sys** | afl.rs (same class; libFuzzer CI integration is smoother) |

## The interesting decisions, argued properly

### QuickJS-ng over V8 — the defining trade

V8 is the fastest JIT on the planet and defines the Speedometer benchmark.
It also defines the memory floor of every Chromium-based browser: multiple
isolates, a huge GC, and a build system that dominates CI time. QuickJS-ng
(the community-maintained QuickJS fork, actively developed through 2026)
delivers interpreter-tier performance at ~1 MB of engine state per realm,
instant startup, and *deterministic* allocation limits — which is exactly
what a memory-budgeted multi-tab engine wants. For interactivity-heavy
pages (DOM churn, events, fetch orchestration) the interpreter gap is
rarely the bottleneck; the layout/paint pipeline usually is. `rquickjs`
0.14 provides a safe, mature binding layer with bundled QuickJS-ng, native
async, and per-realm memory limits — we hard-cap each realm at 256 MiB and
1 MiB stack, with an interrupt hook for runaway scripts.

### A custom selector matcher over lightningcss's AST

Stylo's `selectors` crate is the gold standard, but embedding it means
implementing its full `Element` trait against Stylo's assumptions and
keeping the crate version bit-identical to lightningcss's vendored fork
(`parcel_selectors`). Both are solvable; the third problem is that
lightningcss's `SelectorImpl` is private, which forces a fork. The 2026
compromise: parse with lightningcss (so the grammar, error recovery and
serialization are battle-tested) and match against the `parcel_selectors`
component AST ourselves. The matcher supports the selector subset that
covers the overwhelming majority of real stylesheets — type, class, id,
attribute operators, descendant/child/sibling combinators, `:is()`,
`:where()`, `:not()`, `:nth-child(an+b[ of S])`, `:root`, `:empty`,
`:link/:visited` (visited never matches — history sniffing) — and it is
property-tested and fuzzed. Full Stylo parity is a roadmap item with a
clear seam (`brows12-css::matcher`).

### Hand-rolled DNS-over-HTTPS

This looks like the one place we violated "reuse over rewrite", and it is —
deliberately. The engine needs exactly one DNS operation: resolve A/AAAA +
*the CNAME chain* for cloaking detection, over HTTPS, using a transport we
already own (the hyper client). hickory is a full recursive resolver with
its own connection pooling, config formats and runtime integration; wiring
it in for one GET would add a second async stack to the binary. Our RFC
8484 client is ~200 lines, uses the standard base64url wire encoding, and
the response parser (including name compression) is a dedicated fuzz target
— the classic memory-safety traps are exactly where we put the harness.

### tiny-skia now, vello later

vello is the future of GPU 2D rendering and it is on the roadmap. Today it
would make every CI run a driver matrix (Vulkan/DX12/Metal) and add wgpu's
cold-start cost to a browser whose selling point is instant startup.
tiny-skia is a CPU rasterizer of Skia quality: deterministic output across
platforms, zero driver dependencies, and fast enough for 1280×720 page
paint in single-digit milliseconds on modest hardware. The compositor layer
(`brows12-render::compositor`) is the seam where GPU tiles plug in.

### redb over sled

The engine needs a small set of durable KV tables (web storage, cache
index, cookie snapshots). redb is pure Rust, ACID, actively maintained
(4.x line), and its single-file design fits the per-profile model. sled's
maintenance status has been uncertain for years; rocksdb and sqlite drag
in C builds on every platform for KV workloads that redb serves at
microsecond latencies.

### jiff over chrono

jiff is BurntSushi's ground-up datetime library: DST-safe arithmetic,
RFC 3339/2822 handling, no panics on invalid input. The engine uses it for
cookie/cache expiry bookkeeping alongside `httpdate` (which does exactly
one thing: HTTP-date parsing).

## Dependency hygiene

- Zero C/C++ build dependencies except QuickJS-ng (bundled and built by
  `rquickjs-sys` with the `cc` crate) and ring's assembly.
- No OpenSSL anywhere in the tree.
- Feature-gated heavies: `http3` (quinn/h3), `persist` (redb).
- `cargo tree -d` reviewed per release; workspace resolver v2.
