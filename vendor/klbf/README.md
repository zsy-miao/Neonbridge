<p align="center"><img src="docs/klbf/logo.svg" width="120" alt="KLBF Lab"></p>
<h1 align="center">KLBF Lab</h1>
<p align="center">
  <img src="https://img.shields.io/badge/version-Lab%2035-1f46c8" alt="version: Lab 35">
  <img src="https://img.shields.io/badge/prologue-~7%20fps-1f46c8" alt="prologue: about 7 fps">
  <img src="https://img.shields.io/badge/license-GPL--3.0-555" alt="GPL-3.0">
</p>

<p align="center"><b>Played from a new game to the open world: Lucia's prologue and Jason's first mission ("Gone Fishing") completed, free roam from Jason's house, on an AMD Radeon RX 9070 XT.</b></p>

An open lab that runs the Xbox Series X|S (Scarlett) build of the game natively on a Windows PC: a GDK compatibility layer (`kernelx`, `xgameruntime`, `d3d12_xs` → host D3D12, RDNA2 → DXIL shader translation) that combines the best fixes from open compatibility projects into one tested build. Our own fixes (AMD Radeon stability and rendering, water physics, pre-building pipelines, optional AMD FSR frame generation) and fixes from other contributors, credited below.

This is the one version we recommend. It is meant for every graphics card; so far we have tested it on an AMD Radeon RX 9070 XT, and reports from other cards (NVIDIA, Intel, other AMD) are very welcome. New builds are tried first on the **[`beta` branch](https://github.com/PastiiJay/klbf-lab/tree/beta)** of this repository (test builds); issues and test reports go in [this repository's issues](https://github.com/PastiiJay/klbf-lab/issues).

## Status (2026-11-25, Lab 35)

Tested on one machine: AMD Radeon RX 9070 XT 16 GB (Adrenalin 32.0.32029.2010), AMD Ryzen 7 9800X3D, 32 GB RAM, Windows 11. Self-reported, one machine so far.

| Stage | Status | Notes |
|---|---|---|
| Boot, first-run screens, intro, menus | ✅ Works | 30 fps |
| In game | ✅ Player in control | the player controls the character from the prison prologue up to Jason's first mission |
| Frame rate, prologue (prison, in the cell) | ✅ ~7.3 fps | standing in Lucia's cell: average 7.34, 1% low 7.0, peak 7.7 over 72 min |
| Frame rate, prologue while playing (cell → office → robbery → arrest) | 🟡 ~5.7 fps | average 5.7, median 5.7, peak 9.4 |
| Frame rate, Jason's first mission ("Gone Fishing", on the boat) | 🟡 ~6.3 fps | average 6.3, median 6.5, peak 9.8 |
| Settings | — | renders at 1280×720 (`--console series-s`), upscaled to 1920×1080 with AMD FSR; dips to ~0.5–1 fps while a new area loads |
| Frame rate in game, native resolution | 🟡 ~5 fps | about 200 ms per frame |
| Stability | ✅ 74 min in one session without a crash (this version) | recommended settings, still camera; 77 min of play without a crash on test build Lab 39 |
| Rendering | 🟡 Mostly right | walls, lighting, hair and outfits drawn; purple tint on small flat objects (posters, stickers, towels), some faces deformed (guards, bank), faint diagonal lines on surfaces, the boat sits on the seabed (water physics fixed in test build Lab 39); the HUD (subtitles, objectives, prompts) stays hidden with the recommended settings in this version; fixed in test build Lab 38 (KLBF Lab Beta); at night the vegetation turns magenta and the ground stays very dark |
| Story progress | ✅ Open world reached: prologue and Jason's first mission ("Gone Fishing") completed, 3% of the story | played from a new game on our own build, with our own saves (no save file from anyone else): Lucia's prologue, then all of "Gone Fishing", then free roam from Jason's house, in a 77 min session without a crash (test build Lab 39); autosave and manual saves work, and the bed moves the time between day and night |

### Why AMD FSR

The recommended settings render the world at 1280×720 and upscale it to 1920×1080 with **AMD FSR** (FidelityFX Super Resolution). The game itself does the same on console: it ships with its own FSR upscaler and calls it every frame. KLBF Lab hands that call to AMD's official FidelityFX SDK on the PC, so the picture is upscaled the way the game expects, and the frame rate goes up by about a third. FSR works on AMD, NVIDIA and Intel cards. Native resolution without FSR is listed too (~5 fps).

## Progress

How far the project gets in the story, scene by scene (one machine: AMD Radeon RX 9070 XT, recommended settings with AMD FSR). Frame rates are measured in the world, while playing, unless stated.

| Part of the story | Reached | Frame rate (average / peak) | Notes |
|---|---|---|---|
| Boot, intro video, start screen, menus | ✅ | 30 fps | menus stay black with the recommended settings: press A |
| Prologue: Lucia's cell | ✅ | 7.3 standing / 5.7 while playing, peak 9.4 | walls, lighting, outfit right |
| Prologue: the office interview | ✅ | (prologue while playing: 5.7) | faces mostly right, some deformed |
| Prologue: the robbery | ✅ | (prologue while playing: 5.7) | purple stickers on the glass, bank faces deformed |
| Prologue: the escape and the arrest | ✅ | ~4 outside, high-contrast light | the earlier crash point, passed since Lab 33 |
| Jason's first mission ("Gone Fishing"): the boat | ✅ (test build Lab 39) | 6.3 / 9.8 | the boat floats and drives since Lab 39; on this version (Lab 35) it sits on the seabed: fail the checkpoint 3 times and the game offers to skip it |
| "Gone Fishing": on foot in the swamp, the camp, finding Donnie | ✅ | 5.2 average / 12.9 peak (41 min session, test build Lab 38) | first fight works; with Lab 39 the character climbs out of the water normally |
| "Gone Fishing": the seaplane, the water landing, the night drive home | ✅ (test build Lab 39) | ~5.2 average / 12.5 peak over the 77 min session; ~9 in flight | mission completed; at night the vegetation turns magenta and the ground stays very dark |
| Jason's house, first manual save | ✅ (test build Lab 39) | — | the bed saves the game; the save menu shows "Gone Fishing (3%)" |
| Open world (free roam) | ✅ (test builds Lab 39 / Lab 42) | ~5–8 while driving and walking | free roam from Jason's house; sleeping in the bed moves the time on, so day and night can both be tested |
| Further | ⏳ next | — | the next missions |

### Milestones

| Date | Build | Milestone |
|---|---|---|
| 24 Nov 2026 | Lab 03 | in game, black world, ~1 fps, controller works |
| 24 Nov 2026 | Lab 23 | first gameplay scene drawn on AMD, ~2 fps |
| 24 Nov 2026 | Lab 26 | AMD memory leak and purple blocks fixed, ~4.4 fps |
| 25 Nov 2026 | Lab 28 | Lucia's face, hair and outfit drawn |
| 25 Nov 2026 | Lab 31 | the cell drawn with its walls |
| 25 Nov 2026 | Lab 35 | 74 min in one session without a crash; prologue ~7.3 fps in the cell |
| 25 Nov 2026 | Lab 37 | played from the prologue to the start of Jason's first mission ("Gone Fishing"), peak 9.8 fps |
| 25 Nov 2026 | Lab 38 (test) | menus, map, HUD, subtitles and button prompts show in Series S mode; 41 min of play without a crash; "Gone Fishing" played up to the seaplane, peak 12.9 fps |
| 25 Nov 2026 | Lab 39 (test) | water physics fixed: "Gone Fishing" completed, 3% of the story, first manual save, 77 min of play without a crash; open world reached from a new game |

How we measure: the frame rate the layer logs about every 5 s, in the world only (loading screens and the first 2 minutes of shader compiling left out). Numbers are given per scene: standing in the prison cell, and while actually playing through the prologue and Jason's first mission (18 min session, test build Lab 37). Lows are taken from those 5 s averages, so single-frame dips may go lower.

### Water physics: fixed in test build Lab 39

In test build Lab 39 (the [`beta` branch](https://github.com/PastiiJay/klbf-lab/tree/beta)) the boat in Jason's first mission **floats and moves**: with only the throttle held it sailed over a minute (wake, objectives updating) before hitting the bank. The fix has also been confirmed by another user on an NVIDIA GeForce RTX 5070 Ti (16 GB), in their own build. Our own play-through on Lab 39: the boat, the seaplane take-off and flight (~9 fps) and the water landing all work. It will come to this version once the mission has been played to its end.

Boats, the seaplane and the player in water all behave as if there were no water: the boat rests on the seabed, the seaplane "flies" under the surface. The water itself is drawn right. We think the cause is this: on console the CPU and the GPU share memory, and the game's physics reads back the water height the GPU computes each frame; in our layer that result stayed on the graphics card, so the physics read an empty height map. The next build copies those water textures back into the game's memory (ported from Geckomacabre/vx-layer); fixed in test build Lab 39, together with the XOR swizzle mode the height map is stored in (ported from MiamiGame/XTP).

> [!TIP]
> Until the water physics fix reaches this version: when a water checkpoint fails, fail it **3 times** and the game offers to skip it.

## Community and test reports

The project started on **[KLBF Lab Beta](https://github.com/PastiiJay/klbf-lab-beta)** (13 stars, 2 forks), now archived: test builds moved to this repository's `beta` branch on 25 Nov 2026. Reports from testers there:

- [RX 6900 XT (RDNA2), Windows 10](https://github.com/PastiiJay/klbf-lab-beta/issues/2): runs our builds for 19 min without a device removal; with Lab 33 the player's face renders on that card too; it helped us test two fixes.
- [RX 6800 (RDNA2)](https://github.com/PastiiJay/klbf-lab-beta/issues/4): measured our AMD fixes on that card (same speed with them; purple sky and wet-looking skin gone).
- [DirectStorage page lists](https://github.com/PastiiJay/klbf-lab-beta/issues/3): an analysis by another tester, now part of the build.

No game files are included. You need your own legally obtained copy.

## What it fixes

| Problem | Cause we found | Fix |
|---|---|---|
| Memory grows ~240 MB/s until Windows runs out | compute shader `39f0e9aeb156a61c` never leaves the AMD driver's compiler | skipped |
| Purple/magenta blocks stuck on screen | console 32-lane pixel shaders run in 64-lane waves on AMD; NaN piled up in temporal buffers | 32-lane programs run per half wave; NaN/Inf filtered on colour outputs |
| Missing or late textures | texture upload queue fixed at 192 MB / 250 ms per frame | configurable, larger by default (`VICTORY_TEXTURE_QUEUE_MB`, `VICTORY_TEXTURE_QUEUE_MS`) |
| Characters missing or broken, faces absent | two meshlet-cull compute shaders hung on AMD and had to be skipped | upstream VIctory #334 (buffer-state tracking) ported; `fc5e66f8f873d3bb` no longer skipped: Lucia's face, hair and outfit render |
| Compute results not seen by other workgroups | `glc` loads not globally coherent on RDNA | upstream VIctory #1271 ported |
| Lost or misplaced asset loads | DirectStorage page lists read after the title changed them | read at the title's submit (analysis by JappertjeV, [#3](https://github.com/PastiiJay/klbf-lab-beta/issues/3)); `VICTORY_PAGE_LIST=enqueue|submit|late` to compare |
| Cell walls, window frame and posters missing (sky where the wall should be) | pipelines skipped while the world loads, textures streamed in part taken for others | ported from [myokex/victory-amd](https://github.com/myokex/victory-amd): pipelines kept across runs and waited for while a world loads (`VICTORY_PIPELINE_GATE_S`), textures shown from their first loaded mip, lost placed views refreshed (`VICTORY_TEXTURE_SWEEP`), dispatches over 65,535 groups split |
| Walls, window frame and outfit wrong for a whole run, at random | during loading the list of remapped texture ranges overflowed and was thrown away, so which textures got re-checked depended on timing | the list is 32× larger and an overflow triggers a full re-check instead of a drop (found by comparing logs of runs with and without walls) |
| The AMD shader compiler crashes (`amdxc64.dll`) on a few pipelines | driver bug on two pipelines | `victory translate --cache --pipelines` pre-builds every pipeline offline, saves as it goes, finds the crashing ones and skips them in game too |

## What you can expect (tested on an RX 9070 XT, Adrenalin 32.0.32029.2010)

| | Result |
|---|---|
| First cell | walls, window frame, posters and photos drawn; Lucia's face, hair and orange outfit render; inmates in the yard |
| Speed, recommended settings (720p upscaled to 1080p with AMD FSR) | **~7.3 fps (median), 74 min with no device removal**, walls drawn throughout |
| Speed, native resolution without FSR | ~4.8–5.2 fps (≈200 ms/frame); 41 min with no device removal (Lab 33) |
| Skipped shaders | only one left (`39f0e9aeb156a61c`, which never leaves the AMD compiler); the other 47 we used to skip now run on this card |
| Cell walls | drawn in 5 of 6 runs (about half before Lab 35). When they are missing, the window frame turns purple and the outfit dark for the whole run; still under investigation |
| Still wrong | a purple tint on some papers and photos, stretched "web" triangles around some characters that change every frame, some textiles look wet, colour banding in skies |

These are a few runs on one machine: other cards and drivers may behave differently.

## Run

> [!IMPORTANT]
> **Before the first run, get AMD's FidelityFX SDK.** The recommended settings (`--console series-s`) use AMD FSR, which needs AMD's DLLs. Download **FidelityFX SDK v2.4.0** from its official page ([GPUOpen-LibrariesAndSDKs/FidelityFX-SDK, releases](https://github.com/GPUOpen-LibrariesAndSDKs/FidelityFX-SDK/releases)), unzip it, and set `VICTORY_FFX_DIR` to its `Kits\FidelityFX\signedbin` folder (it holds `amd_fidelityfx_loader_dx12.dll`). Without it the log says `amd_fidelityfx_loader_dx12.dll not found` and the game crashes once it starts upscaling. No SDK? Run without `--console series-s` (no FSR, slower).

> [!TIP]
> **New to this? Follow the [beginner's guide](docs/GUIDE.md)**: every step is a command to copy and paste, from installing the tools to playing, with a page-file guide and a troubleshooting table.

### Step by step

1. **Install the tools:** [Rust](https://rustup.rs) and Git. You also need your own copy of the game's Xbox Series files on disk, and 32 GB of RAM or a large Windows page file (a run commits about 20 GB).
2. **Get the code and build it:** `git clone https://github.com/PastiiJay/klbf-lab` (or `git clone -b beta https://github.com/PastiiJay/klbf-lab` for the test build), `cd klbf-lab`, then `cargo build --release`. The program is `target
eleaseictory.exe`.
3. **Get the FidelityFX SDK** (see the box above) for the recommended Series S settings.
4. **Pre-build the pipelines, once.** This compiles all of the game's shaders for your graphics card ahead of time, so the game doesn't stutter or crash on them while you play. It takes about 10 to 20 minutes. If it crashes (the AMD driver sometimes does), just run the same command again: it resumes and remembers the shaders that crashed. Run it again after updating KLBF Lab. Set the same variables as for playing first (step 5), then:

Pre-build command:

```powershell
$env:VICTORY_SHADER_OPT = '0'
$env:VICTORY_SKIP_PIPELINES = (Get-Content docs\klbf\skip-amd.txt -Raw).Trim()
target\release\victory.exe translate --cache --pipelines --jobs 8 <game dir>\shaders.rpf
```

5. **Play** with:

```powershell
$env:VICTORY_SHADER_OPT = '0'
$env:VICTORY_SKIP_PIPELINES = (Get-Content docs\klbf\skip-amd.txt -Raw).Trim()
$env:VICTORY_LATE_WAIT = '1'
$env:VICTORY_WIDE_BACK = '1'
$env:VICTORY_PIPELINE_WAIT_MS = '24'
$env:VICTORY_RELAY_COPY_QUEUE = '0'
$env:VICTORY_FFX_DIR = '<FidelityFX SDK>\Kits\FidelityFX\signedbin'
target\release\victory.exe run --console series-s <game dir>\<the title's exe> --execute
```

`--console series-s` makes the title render at 1280×720, and AMD FSR (from the official [FidelityFX SDK](https://github.com/GPUOpen-LibrariesAndSDKs/FidelityFX-SDK)) upscales it to 1920×1080: that is the ~7 fps setting. Without `--console series-s` the title renders at its native resolution (~5 fps here).

> [!TIP]
> With the recommended settings the start screen and the main menu stay **black**: the title is running behind it. Press **A** on the controller (or **Enter** on the keyboard, with the game window focused) to get past them; the picture appears once the world loads. Plug the controller in **before** starting: plugging it in during play pauses the title.

If the device is lost about a minute after the world starts loading (reported on an RX 9070 non-XT, [#6](https://github.com/PastiiJay/klbf-lab-beta/issues/6)), try `VICTORY_LATE_WAIT=0`.

`VICTORY_SHADER_OPT=0` turns off the fork's shader clean-up passes, which crashed the AMD compiler for us. `VICTORY_TEXTURE_BUDGET_MB=6144` fixed some dark textiles for another tester but hid the cell walls in both our runs with it, so we leave it off for now. 

Please report results in an [issue](https://github.com/PastiiJay/klbf-lab/issues): GPU, driver, how long it ran, fps in the window title, and what looks wrong. Patch notes: [CHANGELOG.md](CHANGELOG.md). Full run log: [docs/klbf/results.md](docs/klbf/results.md).

## Credits

**Code and fixes in our builds come from these people and projects, thank you:**
- [@Marvisak](https://github.com/Marvisak) for [VIctory](https://github.com/Marvisak/VIctory), and [@fxceless](https://github.com/fxceless) (hyphennn) for [victory-fork](https://github.com/fxceless/victory-fork), the base of KLBF Lab.
- [@myokex](https://github.com/myokex) ([victory-amd](https://github.com/myokex/victory-amd)): missing walls on AMD.
- [@Geckomacabre](https://github.com/Geckomacabre) (vx-layer) and [@MiamiGame](https://github.com/MiamiGame) ([XTP](https://github.com/MiamiGame/XTP)): the water physics fix (test builds).
- [@BrokeAsh](https://github.com/BrokeAsh): the RDNA 4 shader compiler workaround; [@gmankab](https://github.com/gmankab) (with AugustoZFJ's original fix): Wine pipeline caching and docs; [@HandshakeHackin](https://github.com/HandshakeHackin): test tools and fixes.
- Upstream VIctory pull requests #334, #1271, #1599 and #1696, and the DirectStorage analysis by JappertjeV.

**Testers and reporters:** [@clems206](https://github.com/clems206), [@TG0000](https://github.com/TG0000), [@BlackBu7let](https://github.com/BlackBu7let), [@kzrts](https://github.com/kzrts), and [@HishamMuhammed](https://github.com/HishamMuhammed), whose questions shaped the [beginner's guide](docs/GUIDE.md).

KLBF Lab started from [VIctory](https://github.com/Marvisak/VIctory) by Marvisak and the [fxceless/victory-fork](https://github.com/fxceless/victory-fork) (see [README.fxceless.md](README.fxceless.md)); it is GPL-3.0 like them, and keeps their copyright notices. Fixes ported from other projects are credited where they are used, above and in the commit messages.

Product names are only used to say what this works with; they belong to their owners. KLBF Lab is not affiliated with them. License: GPL-3.0.
