<p align="center"><img src="https://github.com/realblastex/flamingo/releases/download/progress-2026-11-25/flamingo-512.png" width="160" alt="Flamingo"></p>

# Flamingo

Flamingo is a work-in-progress fork of [victory-fork](https://github.com/fxceless/victory-fork),
which is itself a fork of [VIctory](https://github.com/Marvisak/VIctory). VIctory is a
compatibility layer that runs an Xbox Series title natively on Windows from files you supply.
Flamingo is tuned for one freshly released open-world Xbox Series title, run from a dump of your
own legally obtained copy, on an NVIDIA RTX 4070.

**This repository contains no game files, code or assets.** You need your own copy. Flamingo is
not affiliated with or endorsed by any game's developer or publisher, or by Microsoft.

## Status (2026-11-25)

Tested on one machine: Windows 11, NVIDIA RTX 4070 12 GB, Series S profile (`--console series-s`).

| Stage | State |
|-------|-------|
| Boot, menus, start screen | work (30 fps) |
| Opening cutscene | plays through; faces and hair show a dither pattern, eyes come out white, and much of it is too dark |
| First gameplay scene | renders with the player in control: **about 14 fps** facing the window; **9-12 fps** turned fully around toward the room, where the card is 80-85% busy; 4 fps for a few seconds while turning, as the room's textures stream in |
| Shadowed areas | dark: the title's ray-traced bounce light and shadows do not run yet |
| Geometry, characters | some geometry is missing in the first area and some character assets load wrong; under investigation |
| Latest tree | victory-fork main is merged in (run 84): the cell window's wire grate now draws, but the frame rate is lower for now (9-10 fps, first measurement with a cold shader cache) and is being worked on |
| Stability | 15-minute runs end without a device loss |

<p align="center">
<img src="https://github.com/realblastex/flamingo/releases/download/progress-2026-11-25/prologue-window.png" width="70%" alt="The first gameplay scene">
</p>

Screenshots and the logo are attachments of the
[progress release](https://github.com/realblastex/flamingo/releases/tag/progress-2026-11-25), not
files in the tree: the asset policy ([docs/asset-policy.md](docs/asset-policy.md)) keeps
everything derived from the game out of the repository. Every change and every measured run is
logged in [FLAMINGO.md](FLAMINGO.md).

## What Flamingo adds over victory-fork

Flamingo's own work:

- **Stats overlay** (RivaTuner-style): fps and frame time, frames, draws, CPU, GPU and
  copy-back time per frame, VRAM, RAM, and a frame-time graph. **F9** shows or hides it.
  `FLAMINGO_OSD=1` shows it from the start; `FLAMINGO_OSD=0` turns it off.
  <img src="https://github.com/realblastex/flamingo/releases/download/progress-2026-11-25/stats-overlay.png" alt="Stats overlay">
- **Discord Rich Presence**: the game's name, menu or world, frame rate and time played.
- **DualSense support**: a PlayStation 5 controller (and the DualSense Edge), over USB or
  Bluetooth, is the title's first pad while no Xbox controller is connected. It is read over HID,
  so Steam Input and DS4Windows are not needed. Buttons map by position (cross is A, circle is B,
  and so on); Options is Menu, and Create or the touch pad click is View. `FLAMINGO_DUALSENSE=0`
  turns it off. Rumble and the light bar are not driven yet.
- **Faster submissions**:
  - title-memory copies that recover from a fault instead of a kernel call per copy;
  - table constant views read the buffer's own GPU copy, with no CPU fill and no wait;
  - host objects are released on a thread of their own;
  - fewer clock reads per frame.
- **A lean build**: the launcher build has no diagnostics counters, which is about 9% faster.
  The `keys` feature keeps the scripted keys for timing runs.
- **Intro and menu flicker fix**: recently changed guarded textures are hashed at every check.

Pull requests merged from victory-fork and VIctory that have not landed upstream yet:

- victory-fork #665: copy-back fetches go on a copy queue of their own.
- victory-fork #670: no write-watch asking for known view memory.
- victory-fork #799: 64 KiB constant-view clamp and translator fixes. The loose-LDS fallback is
  opt-in here (`VICTORY_WAVE_LDS_LOOSE=1`) because it made the world grainy.
- victory-fork #812: unmapped view blocks are parked instead of asked about every frame
  (+1 fps).
- victory-fork #798: the Series S profile keeps the game's own upscaler instead of FSR 4
  (+2 fps).
- victory-fork #823: four refused compute programs run on 32-lane GPUs, and the ray-tracing
  structure builder has all its pipelines.

- victory-fork #768 (opt-in, `VICTORY_WIDE_TEXTURES=1`): a submission's texture checks run
  several at a time.

Experiments, all off by default, kept for testing (each has its result in FLAMINGO.md):

- `VICTORY_AMBIENT_STUB=1`, `VICTORY_SHADOWMASK_STUB=1`: stand-ins for the ray-traced lighting
  passes, ported from [ScarlettPort](https://github.com/ZigZagBoi1/ScarlettPort) (GPL-3.0). Here
  they did not brighten the dark areas.
- `FLAMINGO_BVH_MISS=1`: every ray-tracing node test misses. The cutscene gets brighter, but one run
  hung.
- `FLAMINGO_CARD_FILLS=1`: reads of memory that shaders wrote lately are made on the card. Slower.
- `VICTORY_AMBIENT=<level>`: after the indirect-light resolve, its output is replaced by a constant
  (from [Belobug's fork](https://github.com/Belobug/VIctory-Belobug-AMD), GPL-3.0). At 0.6 the
  opening cutscene washed out grey.
- `FLAMINGO_QUAD_HELPERS=1`: in pixel shaders, reads of the quad's other lanes reach helper lanes
  (also from Belobug's fork). Sharper fine detail, about 18% slower.

The full list, with the runs behind each one, is in [FLAMINGO.md](FLAMINGO.md).

## Build and run

You need Rust (stable) and Windows 11.

```
cargo build --profile diag -p victory
```

Run it with the settings this build is tuned for:

```
set VICTORY_NO_WATCHDOG=1
set VICTORY_PIPELINE_WAIT_MS=24
set VICTORY_RELAY=1
set VICTORY_LATE_WAIT=3
set VICTORY_CARD_BUFFERS_MIN_MB=1
set VICTORY_CARD_BUFFERS_MB=1024
set VICTORY_INDIRECT_WINDOW=1
set VICTORY_SHADER_OPT=0
set VICTORY_WIDE_MERGE=1
set FLAMINGO_OSD=1
target\diag\victory.exe run <path to your dump>\<the title's executable> --execute --console series-s
```

The first launch after a build translates and compiles shaders for a few minutes. Later launches
use the cache.

For Discord Rich Presence, put your own Discord application id in
`%LOCALAPPDATA%\flamingo\discord_app_id.txt` (FLAMINGO.md has the steps).
`FLAMINGO_DISCORD=0` turns it off.

## Reporting a problem (logging build)

When something goes wrong (a black screen, a crash, missing pictures), a log tells us what
happened on your PC. `flamingo-log.cmd` makes one:

1. Install [Rust](https://rustup.rs) and [Git](https://git-scm.com) if you have not.
2. Get the newest code: `git clone https://github.com/realblastex/flamingo` (or, in a clone you
   already have, `git pull`).
3. Drag the title's executable from your own dump onto `flamingo-log.cmd` in the repository's
   folder, or run it from a command prompt:

   ```
   flamingo-log.cmd "D:\path\to\your\dump\the-title.exe"
   ```

   It builds Flamingo (several minutes the first time), runs it with the settings above and
   its log on, and remembers the path for next time (`flamingo-game.txt`).
4. Play until the problem shows (for a black screen, wait a minute in it), then close the game
   window.
5. The log is saved in the `logs` folder as `flamingo-<date>-<time>.log`, with a `.zip` of it
   next to it (Explorer opens there). Attach the `.zip` to your issue, and say what you saw and
   when.

The log starts with your Windows version, processor, memory and graphics cards with their
driver versions. It also holds file paths of your PC, which can include your Windows user
name: look it over before you share it. `set FLAMINGO_SERIES_X=1` before running uses the
Series X profile instead of Series S.

## Credits

- **[VIctory](https://github.com/Marvisak/VIctory)** by Marvisak and its contributors is the base
  of everything here: the loader, the runtime services, the Direct3D 12 back end and the shader
  recompiler. It is licensed GPL-3.0-or-later, and so is this fork.
- **[victory-fork](https://github.com/fxceless/victory-fork)** by fxceless and its contributors
  did the world rendering and speed work this tree starts from, plus the merged pull requests
  above. The fork's own README is in [docs/victory-fork-README.md](docs/victory-fork-README.md).
- **[victory-amd](https://github.com/myokex/victory-amd)** by myokex has an analysis of the dark
  shadowed areas that pointed at the missing ray-traced light.
- **[ScarlettPort](https://github.com/ZigZagBoi1/ScarlettPort)** by ZigZagBoi1: the opt-in
  stand-ins for the ray-traced lighting passes are ported from its VIctory-based runtime
  (GPL-3.0-or-later).
- **[Belobug's VIctory fork](https://github.com/Belobug/VIctory-Belobug-AMD)**: the opt-in ambient
  pass and quad helper reads are ported from it (GPL-3.0).
- The mesh primitive edge-flag fix came as pull request #1 to this repository, a port of upstream
  VIctory #1718.
- **[vx-layer](https://github.com/Geckomacabre/vx-layer)** by Geckomacabre (GPL-3.0): the copy-back
  of textures that shaders write and the title reads on the CPU (`VICTORY_TEXTURE_BACK`), which
  keeps boats afloat.
- Pull request #2 by rafaconvaitis: plain copies recover from guard-page faults.
- The audio fixes of victory-fork #959 (upstream VIctory's XMA and audio ring fixes).
- **[XCL](https://github.com/ivall/xcl)** by ivall and its contributors (GPL-2.0-only) was used as
  a reference only. No XCL code is copied here.
- Flamingo's changes were developed with [Claude Code](https://claude.com/claude-code).

## License

GPL-3.0-or-later, as VIctory. See [LICENSE](LICENSE).
