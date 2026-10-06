# GPU Support — rendering backends, the fallback chain, and the AMD-iGPU crash fix

**Status:** shipped in v2.1 Phase 4 · **Root cause:** confirmed against surfman
0.13.0 source · **Real-hardware verification:** see the checklist at the end
(the fix was designed from source analysis; bare-metal confirmation on the
reporter's machine is pending the user's retest).

---

## 1. The crash (v2.1.0-rc11 Windows build)

Reported on a machine with an **AMD integrated GPU**:

```text
surfman: Could not find the NVIDIA and/or AMD GPU selection symbols.
thread 'main' panicked at surfman-0.13.0/src/gl/device.rs:208:13:
assertion failed: !gl_dx_interop_device.is_null()
```

(the reporter transcribed the path/symbol slightly differently; the actual
site is `surfman-0.13.0/src/wgl/device.rs:208:13`.)

### Root cause, line by line

On Windows, servo 0.6.0 (without the `no-wgl` feature) selects surfman's
**WGL backend** (`surfman::platform::windows::wgl`). Its `Device::new()` does:

1. `Adapter::set_exported_variables()` — looks for the
   `NvOptimusEnablement` / `AmdPowerXpressRequestHighPerformance` exports.
   These exist only when the application embeds surfman's
   `declare_surfman!()` macro. brows12 does not, so surfman prints the
   *"Could not find the NVIDIA and/or AMD GPU selection symbols"* line.
   **This is a benign hint**, not the failure — it only means hybrid-GPU
   switching hints are not set.
2. The WGL backend is hard-wired to the **`WGL_NV_DX_interop`** extension:
   every GL surface lives in a D3D11 texture shared through
   `wglDXOpenDeviceNV`. `WGL_NV_DX_interop` is an *NVIDIA-designed*
   extension that AMD and Intel drivers implement inconsistently — on many
   integrated-GPU systems, VMs and RDP sessions the driver exposes the
   entry points but `wglDXOpenDeviceNV` still returns NULL for the D3D11
   device.
3. surfman then executed `assert!(!gl_dx_interop_device.is_null())` —
   **a panic, not an error**. No fallback was possible because the browser
   process was already dying.

So: any machine where the WGL↔D3D interop bridge cannot open (AMD/Intel
integrated GPUs, several VM hypervisors, RDP sessions, some older or
 OEM-branch drivers) crashed the browser at startup. Nothing about the
crash was AMD-specific — it was a design landmine in the WGL path.

## 2. The fix — three layers

### Layer 1: the right backend per platform

Windows builds now switch surfman to the **ANGLE/D3D11 backend** (the servo
crate's `no-wgl` cargo feature, applied via target-specific dependencies in
`servo-host/Cargo.toml` and `ui/Cargo.toml`). This is the same architecture
Chrome, Edge and Firefox ship on Windows:

- Adapter selection is plain **DXGI enumeration** (`IDXGIFactory1::EnumAdapters1`,
  `VendorPreference::Avoid(Intel)` so a discrete GPU wins when one exists,
  falling back to the only adapter otherwise). There is **no** vendor-specific
  selection API and no `WGL_NV_DX_interop` anywhere in the path —
  **Intel integrated, Intel Arc, AMD integrated, AMD Radeon, every NVIDIA
  generation and virtual GPUs are all reached the same way** (each vendor's
  D3D11 WDDM driver).
- GL is provided by ANGLE translating GLES 3 to D3D11; the ANGLE runtime
  (`libEGL.dll` + `libGLESv2.dll`, built from the pinned mozangle fork)
  ships **next to the executable in the release zip** and must not be
  removed.
- When the machine has **no usable GPU at all** (broken drivers, stripped
  VMs), the software lane uses ANGLE over **D3D11 WARP** — Microsoft's CPU
  rasterizer that ships with Windows itself.

Linux keeps the existing EGL/GLX hardware backend; its software lane is
Mesa **llvmpipe** (surfaceless EGL).

### Layer 2: patched surfman (`patched/surfman`)

surfman 0.13.0 is vendored under `patched/surfman` (same pattern as the
other engine patches) and `[patch.crates-io]` points `surfman` at it. Every
init/present-path `assert!`/`expect`/`assert_ne!` that could fire on a
misbehaving GPU/driver is converted into a normal `surfman::Error`:

| Site | Was | Now |
|---|---|---|
| `wgl/device.rs` `Device::new` | `assert!(!gl_dx_interop_device.is_null())` — **the reported crash** | `Err(DeviceOpenFailed)` + `warn!` |
| `angle/device.rs` `Adapter::new` | 6 × `assert!`/`assert_eq!` on DXGI factory/adapter/desc/QI | skip-to-next-adapter or `Err(NoAdapterFound)` / `Err(Failed)` |
| `angle/device.rs` `Device::new` | `.expect()` on `EGL_ANGLE_device_creation`, `assert_ne!` on EGL device/display/init | `Err(RequiredExtensionUnavailable)` / `Err(DeviceOpenFailed)` + `warn!` |
| `angle/surface.rs` `present` | `assert_ne!(SwapBuffers, FALSE)` (device-lost path) | `Err(Failed)` |
| `base/egl/device.rs` libEGL load | bare `panic!("Unable to load…")` | actionable message naming the DLLs and the fix |

### Layer 3: the brows12 fallback chain (`servo-host/src/gfx.rs`)

`gfx` is the only place brows12 creates rendering contexts. Every backend
attempt runs inside `catch_unwind` (surfman is third-party code — any
residual panic must degrade to an error, never abort the browser), each
failure is logged with its precise reason, and the first success wins.

```text
Windows (policy Auto):
  1. angle-d3d11-hardware   ANGLE/D3D11 over the enumerated DXGI adapter
                            (Intel/AMD/NVIDIA/vGPU — any vendor)   [GPU]
  2. angle-d3d11-warp       ANGLE over D3D11 WARP                  [CPU]

Linux (policy Auto):
  1. egl-glx-hardware       EGL/GLX hardware adapter               [GPU]
  2. mesa-llvmpipe          surfaceless EGL over Mesa llvmpipe     [CPU]

The UI additionally composes: if lane 1 fails at startup the shell logs the
reason and continues on lane 2; lane 2 failing too exits cleanly with a
per-lane diagnostic report (exit code 1, no panic, no backtrace).
```

> **About the "CPU rasterizer" stage of the requested chain
> (GPU → ANGLE → software WebRender → CPU rasterizer):** WebRender in the
> servo 0.6.0 stack has no GL-free CPU path — a GL context is mandatory.
> The practical CPU rasterizers in this stack *are* the software GL lanes:
> D3D11 WARP (Windows) and Mesa llvmpipe (Linux). That is what the chain
> above ships. A hypothetical no-GL WebRender (Firefox's swgl) is not
> available to embedders in this engine version and is honestly out of
> scope.

## 3. User-facing controls

| Control | Effect |
|---|---|
| `brows12-ui --software` | Start on the software (CPU) lane; never touch a GPU lane |
| `brows-servo --software` | Same, for the headless CLI |
| `brows-perf --software` | Accepted for script compatibility (headless perf always renders on the software lane) |
| `BROWS12_SET_PREF="gfx.software-rendering=true"` | The `gfx.software-rendering` preference — equivalent to `--software` on every binary (combine with other prefs: `js_mem_max=128,gfx.software-rendering=true`) |

`brows12-ui --help` documents the flags. The pref is a brows12-side knob
(the engine has no such pref) parsed by `servo_host::gfx::software_rendering_pref()`.

Selection logic lives in one function per lane
(`gfx::GfxPolicy::from_flags`) and is unit-tested with mock probes:
success-first, error-fallthrough, **panic-fallthrough (mock GPU failure —
the exact reported crash is reproduced as a test fixture)**, all-fail
clean-`Err`, empty-candidate, flag handling and pref parsing
(`servo-host/src/gfx.rs` `mod tests`).

## 4. Backend capability matrix

| Machine class | Windows lane reached | Linux lane reached |
|---|---|---|
| Intel integrated (HD/Ultra/Iris) | ANGLE/D3D11 (vendor-agnostic DXGI) | EGL/GLX |
| Intel Arc (dedicated) | ANGLE/D3D11, preferred over iGPU by `Avoid(Intel)` | EGL/GLX |
| AMD integrated (the reported machine) | ANGLE/D3D11 — **fixed** | EGL/GLX |
| AMD Radeon (dedicated) | ANGLE/D3D11, preferred over iGPU | EGL/GLX |
| NVIDIA GTX/RTX/Quadro (all generations) | ANGLE/D3D11 | EGL/GLX |
| Virtual machines (Hyper-V/VMware/VirtualBox SVGA) | ANGLE/D3D11 over the virtual WDDM GPU; WARP if none | EGL/GLX over llvmpipe if none |
| Headless / no display | WARP (no window needed) | llvmpipe surfaceless (no X needed) |
| Broken/absent GPU driver | WARP (CPU rasterizer shipped with Windows) | llvmpipe |
| Missing `libEGL.dll` (broken install) | clean pre-check error before any EGL call | clean error naming the packages to install |

## 5. Packaging note (Windows)

`mozangle` links ANGLE's import library **implicitly**, so
`libEGL.dll`/`libGLESv2.dll` are load-time dependencies of `brows12-ui.exe`
and `brows-servo.exe`. They are built into the release zip next to the
executable; deleting them prevents startup entirely (Windows loader error,
before any brows12 code runs). The CI release workflow copies them from the
mozangle build output and the Windows smoke test verifies a real page load
with them present.

## 6. Real-hardware verification checklist

The fix is verified by construction (source-level root cause), by the mock
unit tests, and by CI smoke tests (Linux headless + Linux UI smoke; Windows
WARP smoke). What CI cannot do is exercise real AMD/Intel/NVIDIA drivers.
When hardware access is available, run:

1. **The reported machine (AMD integrated GPU, Windows)** — unzip the
   v2.1.0 release, run `brows12-ui.exe`. Expected: starts on
   `angle-d3d11-hardware`; stderr shows
   `brows12 gfx: rendering backend 'angle-d3d11-hardware' OK`. If the
   driver refuses ANGLE/D3D11 (never observed for WDDM drivers), the log
   must show the automatic fallthrough to `angle-d3d11-warp`.
2. `brows12-ui.exe --software` on the same machine →
   `angle-d3d11-warp` lane, still fully interactive (slower).
3. NVIDIA laptop with Optimus (iGPU + dGPU): discrete GPU should be
   selected (verify in Task Manager → GPU column while loading a heavy
   page).
4. Intel-only desktop, and a Hyper-V/VMware VM without 3D acceleration:
   ANGLE/D3D11 on the virtual adapter, else WARP.
5. RDP session (WGL_NV_DX_interop is typically unavailable over RDP — the
   old build would have crashed here too): expect lane 1 or lane 2 to come
   up cleanly.

Each observation should be appended to `docs/V2_1_PHASE4_REPORT.md` as it
is collected.
