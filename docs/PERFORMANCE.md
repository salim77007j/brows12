# Performance

## Methodology

Numbers in this document come from two sources, both committed to the repo:

1. `brows12-compare` (`benchmarks/src/bin`) — engine-side timings that map
   1:1 to user-visible operations: cold engine construction, tab creation,
   first paint of an internal page, and a full pipeline load against a local
   HTTP server. CI runs it on every commit and appends the JSON to artifacts.
2. Criterion micro-benchmarks (`cargo bench -p brows12-benchmarks`) — HTML
   parse throughput, CSS parse, full-document cascade, layout, raster, JS
   realm operations, cookie jar and cache operations.

Cross-browser comparison uses `benchmarks/scripts/compare_browsers.sh`,
which measures Chrome/Firefox/Brave cold start (`hyperfine`, headless
`about:blank`) and appends JSON per browser. Browser numbers are only
comparable within one machine; the script exists so anyone can reproduce —
we deliberately do not publish cross-browser tables we cannot regenerate.

## Engine-side baseline (debug build, development container, 2 vCPU)

| metric | value |
|---|---|
| cold start — engine construction (TLS stack, font scan) | 23 ms |
| tab creation | < 1 ms |
| internal page (`brows12://`) first paint | 11 ms |
| local page full pipeline (fetch → parse → cascade → layout → paint) | 6 ms |

Release builds (`lto = "thin"`, `codegen-units = 1`) shrink these further;
the CI bench job publishes up-to-date artifacts.

## Where the wins come from

### Startup

- No JIT to warm: QuickJS-ng starts in microseconds per realm.
- No GPU initialization before the first frame (CPU raster path).
- Font system scan happens once per engine, shared by all tabs
  (`Arc<Mutex<FontSystem>>`).
- Everything heavy is lazy per tab; the engine itself wires ~5 subsystems.

### Memory

- Arena DOM: one `Vec<Node>` per document; teardown is O(1) and refunding
  to the allocator is immediate (no Rc cycles to collect).
- Display-list rendering: paint state is flat and pre-sorted; no retained
  layout objects beyond the rect map.
- Tab suspension is the big lever: a suspended tab is a URL string, a title
  and an optional snapshot. The LRU sweep (`Engine::enforce_memory_budget`)
  keeps resident pages within `max_live_pages` automatically.
- Response bodies > 1 MiB skip the memory cache (disk layer still caches).
- JS realms: 256 MiB hard heap cap each, enforced by QuickJS's allocator
  integration — a runaway page fails alone.

### Throughput

- lightningcss parses at multiple hundreds of MB/s on commodity hardware;
  cascade matching is the O(rules × elements) hot path and is benchmarked
  explicitly (`cascade/60_sections_full_document`) so regressions are loud.
- Layout calls into taffy's cached, incremental-friendly solver; text
  measurement is the only external call and is bounded by leaf count.
- tiny-skia paints 1280×720 pages in single-digit milliseconds on 2 vCPU
  (bench `render/60_sections_raster_1280x720`).
- Glyphs render through a swash image cache keyed by (font, glyph, size,
  subpixel bin) — shaping runs once per measurement, blitting runs per
  frame.

## Optimization backlog (tracked in ROADMAP)

- Bloom-filtered rule matching (ancestor hashes) for cascade.
- Incremental relayout (dirty-node taffy updates instead of full rebuild).
- Glyph atlas persistence across frames (avoid re-blit of static text).
- vello GPU tiles behind the compositor seam.
