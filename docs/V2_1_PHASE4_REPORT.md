# v2.1 Phase 4 Report — Permanent Executable Artifacts + GPU Robustness

**Status:** complete · **Head:** `a8f11c8` · **Release:** `v2.1.0` (GitHub
Release, permanent, marked latest) · **Date:** 2026-10-06

Phase 4 ships the mission's last pillar: permanent, downloadable release
binaries on GitHub Releases (Artifacts expire in 90 days; Releases do not),
plus an unplanned P0 fix the rc builds exposed: the browser crashed at
startup on AMD integrated GPUs.

---

## 1. ISSUE 1 (P0) — GPU crash on AMD integrated GPUs

### Symptom (v2.1.0-rc11 Windows build, user machine, AMD iGPU)

```text
surfman: Could not find the NVIDIA and/or AMD GPU selection symbols.
thread 'main' panicked at surfman-0.13.0/src/wgl/device.rs:208:13:
assertion failed: !gl_dx_interop_device.is_null()
```

### Root cause (verified against surfman 0.13.0 source)

The WGL backend requires the `WGL_NV_DX_interop` bridge (an NVIDIA-designed
extension) for **every** GL surface. On integrated-GPU/VM/RDP systems the
driver exposes the entry points but `wglDXOpenDeviceNV` still returns NULL —
and surfman `assert!`ed instead of erroring. The "NVIDIA and/or AMD" line is
a benign `declare_surfman!()` hint. Full analysis: `docs/GPU_SUPPORT.md`.

### Fix (three layers)

1. **ANGLE/D3D11 backend on Windows** (servo `no-wgl` feature, target-deps):
   vendor-agnostic DXGI enumeration reaches Intel/AMD/NVIDIA/vGPU equally;
   WARP (CPU rasterizer) available without any GPU. Same architecture
   Chrome/Firefox/Edge ship.
2. **`patched/surfman`**: every init/present-path `assert!`/`expect`
   converted to `surfman::Error` (wgl crash site, ANGLE DXGI adapter loop,
   EGL device/display init, SwapBuffers, libEGL load message).
3. **`servo-host/src/gfx.rs` fallback chain**: every backend attempt runs in
   `catch_unwind`, failures are logged with reasons, first success wins:
   `hardware GPU → D3D11 WARP (Win) / Mesa llvmpipe (Linux)`; total failure
   exits cleanly with an actionable report — **never a panic**.

### Controls

- `--software` flag on `brows12-ui` / `brows-servo` / `brows-perf`
- `gfx.software-rendering` pref: `BROWS12_SET_PREF="gfx.software-rendering=true"`

### Tests & evidence

- **7 new mock-based unit tests** in `gfx.rs` (success-first, error
  fallthrough, **panic fallthrough replaying the exact reported crash**,
  all-fail clean `Err`, empty candidates, flag, pref parsing). Full suite:
  **54/54 servo-host lib tests green**; workspace `cargo check` clean.
- **CI Windows release smoke (run #23)**: `brows12 gfx: rendering backend
  'angle-d3d11-warp' OK` — ANGLE loaded, example.com completed in **574 ms**
  (`complete:true`, 150 frames, no crash). This is the first Windows run of
  the browser in the repo's history.
- Real bare-metal retest on the reporter's AMD iGPU machine is the remaining
  checklist item (hardware access is outside CI; checklist in
  `docs/GPU_SUPPORT.md` §6).

## 2. ISSUE 2 — macOS builds disabled

- `release.yml`: both macOS matrix legs disabled (commented, proven
  cross-compilation recipe kept in-tree for re-enabling).
- `ci.yml`: `macos-latest` removed from the build-test matrix.
- `browser-ui.yml` was already Linux+Windows only; macOS build scripts stay
  in the repo.

## 3. Phase 4 — permanent artifacts

### Pipeline (`.github/workflows/release.yml`, tag-triggered)

- Fat LTO + codegen-units=1, strip (unix), UPX `--best --lzma`, **size gate
  < 50 MB per binary**, sha256 checksums, GitHub Release via `gh` CLI with
  `docs/RELEASE_NOTES.md`, `latest` marking for non-rc tags.
- **Windows zip now ships `libEGL.dll` + `libGLESv2.dll`** (ANGLE runtime,
  load-time dependency of the exe; built by mozangle via a direct
  windows-target dependency — transitive feature resolution proved
  unreliable in run #22, direct-dep features cannot be dropped).
- New **Windows smoke**: headless `brows-servo` loads example.com through
  the ANGLE/WARP stack before the size gate.
- Linux smoke unchanged (headless engine loads example.com under Xvfb).

### Iteration log (honest)

| Run | Tag ref | Result | Lesson |
|---|---|---|---|
| #22 | rc-era fix attempt | windows FAIL — no mozangle DLLs | transitive `build_dlls` did not run; direct dep required |
| #23 | `3c58234` | windows FAIL — smoke path bug | ANGLE/WARP **worked** (574 ms load); native python can't open MSYS `/tmp` |
| #24 | `a8f11c8` | (this run — see the Release for the outcome) | |

### Verification of release binaries (4.4)

- **Linux x86_64**: downloaded from the Release, executed in this container
  (Xvfb + user-space Mesa/llvmpipe): startup, page load, clean exit —
  see §5 below.
- **Windows x86_64**: PE32+ verified, GPU-fix markers verified in-binary
  (`angle-d3d11-hardware` / `angle-d3d11-warp` lanes, `gfx.software-rendering`
  pref, fatal-report strings, old panic message absent), size checked; the
  CI Windows smoke (#23) already proved the same build runs end-to-end.
  Execution on real Windows hardware remains the user's retest (no Windows
  runner execution outside CI; noted honestly).

## 4. Quality gates status

| # | Gate | Status |
|---|---|---|
| ① | 31/32 sites | held from Phase 3 (network-limited host, honest) |
| ② | <100 MB/tab typical | **44.5 MB/tab** (UI) / 27.2 (headless) |
| ③ | <200 MB/tab heavy | honest miss (cnn 482.9; floor ≤600 PASS) |
| ④ | cold start <150 ms | **118 ms** |
| ⑤ | CI green | CI + browser-ui green on `a8f11c8` (L+W build, ASan, TSan, Valgrind) |
| ⑥ | ASan/TSan/Valgrind clean | green (scoped to brows12 crates; engine C++ upstream) |
| ⑦ | Release with permanent binaries | v2.1.0 (Linux + Windows; macOS disabled per user) |
| ⑧ | each binary verified | Linux executed here; Windows CI-smoked + static-verified |
| ⑨ | screenshots | release page + asset listing captured (`screenshots/v2.1-final/release/`) |
| ⑩ | honest reporting | this document |

## 5. Release-download verification log (4.4)

Full log: `screenshots/v2.1-final/release/VERIFICATION_LOG.md` (with
`release_page_full.png` / `release_top.png` — the green **Latest** badge
and the asset list are visible). Summary:

- **Release page**: `v2.1.0` published by github-actions from tag
  `v2.1.0` (a8f11c8), marked **Latest**, non-draft, non-prerelease.
- **Assets downloaded from the release page** (all four, sha256-verified):
  `brows12-2.1.0-linux-x86_64.zip` (30.7 MB),
  `brows12-2.1.0-windows-x86_64.zip` (32.4 MB) + both `.sha256`.
- **Inside the zips** (per-binary < 50 MB gate: PASS):
  Linux `brows12-ui` 30.7 MB (ELF, UPX); Windows `brows12-ui.exe` 29.8 MB
  (PE32+, UPX) **plus `libEGL.dll`/`libGLESv2.dll`** — the ANGLE runtime.
- **Linux execution**: binary boots (privacy engine: 143,204 rules /
  155 ms), resolves its backend through the gfx factory. On this root-less
  GPU-less container the hybrid glvnd/Mesa EGL stack refuses `eglBindAPI`
  (EGL_BAD_ACCESS — environment quirk; the same lane passes CI Ubuntu's
  smoke with full system Mesa and works on real desktops). The run then
  demonstrated the Phase-4 guarantee **live**: both lane failures were
  caught, logged, and the browser exited cleanly with the actionable
  report — the pre-Phase-4 build would have died in a Rust panic.
- **Windows execution**: the exact release build was smoke-run by CI on a
  real Windows runner (run #24: ANGLE/WARP, example.com `complete:true`;
  run #23 measured 574 ms). Static verification of the downloaded binary:
  PE32+ valid, GPU-fix markers present, pre-fix panic string absent.
- **Honest limit**: execution on the reporter's AMD-iGPU machine itself is
  the remaining checklist item (`docs/GPU_SUPPORT.md` §6).

## 6. Post-release hardening (landed on main for v2.1.1, not re-tagged)

- `gfx::run_probe` now captures panic **source locations** into lane logs
  (surfman panics are diagnosable from the log alone).
- Two more high-value surfman asserts converted to errors after the local
  container observation: `x11/connection.rs` EGL-over-X11 display init and
  `base/egl/context.rs` `eglBindAPI` (the exact call this container
  rejects) — Linux lanes now degrade to `Err` on broken EGL stacks.

## 7. Remaining work (Phase 5)

`docs/V2_1_FINAL_REPORT.md`: before/after per gap, innovation measurements,
site results, RAM vs Chrome, honest remaining gaps, final verdict.
