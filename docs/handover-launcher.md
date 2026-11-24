# Handover: launcher, window and release session

Everything this session finished is committed and pushed; nothing of it is left uncommitted. The modified d3d12 files in the tree (page_mapping.rs, device.rs, device/views.rs, device/views/shadow.rs) belong to the d3d12 session, not this one.

## Borderless fullscreen: state

- 43aca47 / 65fc79e added these:
  - a borderless popup at creation, covering rcMonitor;
  - an Alt+Enter/F11 toggle and the WM_SYSCHAR swallow;
  - TUXBOX_MONITOR=primary|nearest|N;
  - TUXBOX_CLIP_CURSOR=1;
  - per-monitor DPI v2;
  - MakeWindowAssociation(NO_ALT_ENTER);
  - the minimised present skip and OCCLUDED -> S_OK;
  - a DirectComposition letterbox (d3d12 present.rs, `Letterbox`).
- After the user saw a black window, every new behaviour was made opt-in:
  - letterbox: TUXBOX_LETTERBOX=1 (77363a9);
  - borderless at creation: TUXBOX_FULLSCREEN=1 or NEONBRIDGE_WINDOWED=0 (edc5752);
  - DPI v2: TUXBOX_DPI_AWARE=1 (e5ea18a);
  - minimised skip and OCCLUDED pace: TUXBOX_PRESENT_MINIMISED=0, TUXBOX_OCCLUDED_STATUS=0 (091353a).
- These stay on by default:
  - Alt+Enter/F11 toggle (TUXBOX_ALT_ENTER=0 turns it off);
  - SYSCHAR swallow;
  - MakeWindowAssociation;
  - the windowed fit uses the target monitor's work area instead of the primary SPI_GETWORKAREA;
  - after the first present, the uncloak shows, raises and foregrounds the window and logs `window: uncloaked 0x.. (brought to the front: ..)`.
- Symptom (debugger, test38/39, and the user's screenshot):
  - the borderless window is in front and the FPS overlay shows 15 fps;
  - the title only clears: draws=0 and lit=0 in every sample;
  - test37, before borderless, drew the loading art;
  - the present and letterbox paths are fine.

## Draws=0: A/B plan (sent to the debugger, not yet run)

Run on 091353a or later, short timeouts, comparing draws= and lit:

- A. Default. Expect draws>0. If draws are still 0, suspect what stays on: the monitor-based fit, MakeWindowAssociation, F11/SYSCHAR, the foreground raise, or others' d3d12 changes.
- B. TUXBOX_FULLSCREEN=1: the popup alone. Its client is 1920x1080 against a 3840x2160 chain.
- C. TUXBOX_FULLSCREEN=1 TUXBOX_CLOAK_WINDOW=0.
- D. TUXBOX_DPI_AWARE=1.
- E. TUXBOX_PRESENT_MINIMISED=0 TUXBOX_OCCLUDED_STATUS=0.

Leading theory: the title reads its client or monitor size, or the window style, and changes its render path when it sees a monitor-sized popup. If B alone reproduces it, try creating the popup at the title's requested size and restyling to full-monitor after the first present, where xmem uncloaks. Keep restyling off the present path itself (rebox b45025e deadlock).

## Portable release

- eb1617d:
  - a `tuxbox\` folder beside neonbridge.exe holding every provider DLL is used prebuilt: no cargo or MSVC needed;
  - `neonbridge --package-release <dir>` builds a release;
  - `cargo package-release` (alias in .cargo/config.toml) builds into target\package and runs it.
- Zip: target\package\dist\neonbridge-0.1.0-win-x64.zip, built from 091353a.
- Its README.txt still said borderless was the default. The text is fixed in src/main.rs (RELEASE_README, commit after eb1617d). Re-run `cargo package-release` before publishing so the zip carries the fix.
- dxcompiler.dll and dxil.dll ship because they are on the Windows SDK redist list (Redist\D3D, classic apps). dxc.exe is not on that list and is not shipped: the shader worker needs the SDK or a DXC release, set through DXC.
- Not tested on a clean PC.

## Research's shader items (Xbox2Windows): all already covered

- 07f0d40b: fixed-offset MUBUF atomics. Covered at hlsl.rs ~7590-7660, signed forms through CAS.
- 7f3b070f: image_sample_l maps to SampleLevel (modi 4), including _o. c_l is refused unless the LOD is 0 (no SampleCmpLevel in SM5).
- 6ebbcc58: D16 UINT stores, covered by 5f12442.
- 929f6b13: text-spelling only; not applicable to the machine-code decoder.
