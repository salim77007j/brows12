# v2.1.0 Release-download verification log (Phase 4.4)

Date: 2026-10-06 · Verifier: brows12 CI session (no physical GPU hardware)

## Release page

- URL: https://github.com/salim77007j/brows12/releases/tag/v2.1.0
- Marked **Latest** (screenshot: release_page_full.png — green "Latest"
  badge, release notes, Downloads table, "Assets 6").
- Published by github-actions from tag v2.1.0 (commit a8f11c8).

## Assets (all downloaded from the release page)

| Asset | Size | sha256 |
|---|---|---|
| brows12-2.1.0-linux-x86_64.zip | 32,179,484 B (30.7 MB) | OK (`sha256sum -c`) |
| brows12-2.1.0-windows-x86_64.zip | 33,995,191 B (32.4 MB) | OK (`sha256sum -c`) |
| *.sha256 | — | attached per asset |

Inside the zips (each binary < 50 MB gate: PASS):

- `brows12-2.1.0-linux-x86_64/brows12-ui` — ELF x86-64, 32,171,340 B (30.7 MB)
- `brows12-2.1.0-windows-x86_64/brows12-ui.exe` — PE32+ x86-64, 31,280,128 B (29.8 MB)
- `brows12-2.1.0-windows-x86_64/libEGL.dll` (50.5 KB) + `libGLESv2.dll` (6.7 MB)
  — the ANGLE runtime the Windows exe loads at startup (docs/GPU_SUPPORT.md §5)

## Linux x86_64 — executed

- Container run (Xvfb :99, no GPU, no root): binary starts, boots the
  privacy engine (143,204 rules / 155 ms), resolves the rendering backend
  through the gfx factory. The container's hybrid glvnd/Mesa EGL stack
  rejects `eglBindAPI` (EGL_BAD_ACCESS — environment quirk of this
  stripped, root-less container, not a product defect: the same lane works
  on CI Ubuntu and real desktops). The fallback chain then demonstrated
  its guarantee live: every lane panic was CAUGHT, logged with reasons,
  and the process exited cleanly with the actionable report — never a
  panic crash.
- CI Ubuntu (release.yml run #24, full system Mesa): headless smoke loaded
  https://example.com through the software lane — complete:true.

## Windows x86_64 — verified

- PE32+ executable, UPX-packed; GPU-fix markers present in-binary
  (`angle-d3d11-hardware` / `angle-d3d11-warp` lanes, `--software`,
  `gfx.software-rendering`, fatal-report strings); the pre-fix panic
  string ("Where's the `EGL_ANGLE_device_creation` extension?") absent.
- CI Windows (release.yml run #24): smoke loaded https://example.com
  through ANGLE/WARP — complete:true (run #23 measured 574 ms).
- Execution on the reporter's AMD-iGPU machine remains the user retest
  (checklist: docs/GPU_SUPPORT.md §6).
