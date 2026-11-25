# Neonbridge for developers

Everything a player doesn't need: building from source, the command line, the runtime trees and their settings. For playing, see the [README](README.md).

## Building from source

```
cargo build --release                      # target\release\neonbridge.exe
neonbridge                                 # the picker window (or, in a terminal, numbered choices)
neonbridge --runtime victory --game <dump dir> [--log run.log] [--timeout S]
```

You need Visual Studio Build Tools with the x64 C++ tools, the Windows SDK and Rust (stable, `x86_64-pc-windows-msvc`). Each runtime's own toolchain is pinned in its tree (`rust-toolchain.toml`), and rustup fetches it.

`neonbridge --package` builds every runtime and lays out a release. A runtime that can't be built here (a missing prerequisite or a failed build) is left out and listed under "Not bundled" in the package's README.

## Runtimes

Each runtime is a tree under `vendor/<name>`, copied unchanged from its upstream at the hash in its `UPSTREAM` file. Its `NEONBRIDGE.toml` tells the launcher how to build it, which exe and arguments to run, its environment, options, presets and setup steps. A runtime's own fixes, if any, are in `vendor/patches/rt-<name>.patch`, applied for the build and reverted after it.

In picker order: each toml's `rank = N` (best first, by the leader's own published results; unranked trees last, Mix always last).

| Rank | `--runtime` | What it is | Upstream |
|---|---|---|---|
| 1 | `klbf` | KLBF Lab, a VIctory/victory-fork build (AMD-tested; `--preset fsr` = its Series S + AMD FSR setting, needs `VICTORY_FFX_DIR`) | PastiiJay/klbf-lab |
| 2 | `flamingo` | Flamingo, a VIctory fork | realblastex/flamingo |
| 3 | `victory` | VIctory-fxceless, the victory emulator fork | fxceless/victory-fork |
| 4 | `vitoriabr` | VItoriaBr's VIctory fork | uiliannn/VItoriaBr |
| 5 | `xcl` | xcl | ivall/xcl |
| 6 | `belobug` | VIctory-Belobug-AMD, a VIctory fork tuned for AMD | Belobug/VIctory-Belobug-AMD |
| 7 | `scarlettport` (default for `--runtime`) | ScarlettPort's VIctory-based runtime and launcher scripts | ZigZagBoi1/ScarlettPort |
| - | `neonbridge-own` | Neonbridge (own build), hidden from the picker: TuxBox console-library providers staged into the game folder, with Neonbridge's fixes and shader translator | brcly/TuxBox |

Each runtime's default launch is exactly its author's (flags, environment, preset, cited in its toml); anything of Neonbridge's is an optional `preset_*`.

The picker selects the last pick, else `NEONBRIDGE_RUNTIME`, else rank 1.

The loader reads each step's own counts (`src/progress.rs`, one parser test per runtime): cargo's `Building [..] N/M` (asked for with `CARGO_TERM_PROGRESS_WHEN=always`), victory's `warm-up N shaders so far`, xcl's `N/M (t s)`, TuxBox's `label · N/M ready`, ScarlettPort's precompile-graphics records over the last full pass (`graphics-total.txt`), and Five Star's `[i/n] step` and `N/M (p%)` lines. A step with no count shows a pulse, not a percentage.

A build is skipped when its build stamp (`<exe>.nbstamp`: the tree's git hash, its uncommitted changes, its patch and its build line) still matches.

## Commands

| Command | Does |
|---|---|
| `neonbridge` | No arguments: the picker. In a terminal with `NEONBRIDGE_PICKER=0`, nothing is asked. |
| `--runtime <name>` | The runtime to use (or `NEONBRIDGE_RUNTIME`; default `scarlettport`). |
| `--list` | The runtimes. |
| `--list <name>` | That runtime's options, their defaults and the values a run would use, plus its presets and setup steps. |
| `--set NAME=VALUE` | One option for this run: an environment variable, or `--set --flag=VALUE` for the runtime's own arguments. Repeatable. |
| `--preset <name>` | A preset from the runtime's `NEONBRIDGE.toml`, applied before `--set`. |
| `--mix <name>`, `--mix-part <part>=<runtime>` | Build one victory exe from crates of several trees. |
| `--setup <name>` | The runtime's setup steps. A step that downloads something or needs a file of yours asks first and runs only on a typed `yes`. |
| `--game <dir>`, `--exe <file>` | The dump folder, and another title exe in it. |
| `--log <file>`, `--timeout <s>` | Where the run's log goes (default `%LOCALAPPDATA%\6ixpack\logs\<runtime>_<time>.log`), and a time limit. |
| `--build-only`, `--prewarm` | Build the runtime only, or run its shader prewarm. |

Settings apply in this order, the later winning: the toml, your environment, the preset, then `--set`.

The `neonbridge-own` runtime keeps its own options (`--debug`, `--rev`, `--tuxbox`, `--pipelines`, `neonbridge ctl ...`); `neonbridge --runtime neonbridge-own --help` lists them.

## Environment variables

| Variable | Does |
|---|---|
| `NEONBRIDGE_LOG=0` | No run log (logging is on by default for every runtime; children get `RUST_LOG=info`, the victory family also `VICTORY_LOG=info`). |
| `NEONBRIDGE_DIAG=0` | No diagnostics build. On by default: each toml's `diag_build`/`diag_exe` (the author's own diagnostics build) is used, with its own build stamp. The picker's Diagnostics box sets it. |
| `NEONBRIDGE_FEED=1` | Stream the run over Tailscale (`tools/tsproxy.exe`, else `tools/cloudflared.exe`) behind a site password; needs ffmpeg. Shows the loader and the game window. |
| `NEONBRIDGE_FEED_WINDOWS=0` | Stream only neonbridge-own's frame tap, not window captures. |
| `NEONBRIDGE_AUTO_A=0` | Turns off auto-press A: by default the launcher focuses the game window and presses Enter (A) every 8 s for the whole run, alternating a tap (start screens) and a 6 s hold (cutscene skip), so unattended runs don't stall (`NEONBRIDGE_AUTO_A_EVERY`, `_FOR` in seconds, `_HOLD_MS`, `_LONG_MS`) |
| `NEONBRIDGE_CACHE_BACKUP=0` | Turns off the cache backup: at each launch and after each shader warm, the runtime's shader and pipeline cache is mirrored (new files only, never deleted) into `%LOCALAPPDATA%\6ixpack\cache-backup` at idle priority; `=<folder>` backs up there instead |
| `NEONBRIDGE_RESOLUTION=1920x1080` | Render size for the victory family (sets `VICTORY_DISPLAY_MODE`; default 3840x2160) |
| `NEONBRIDGE_FPS_CAP=30` | Frame cap where the runtime has one (sets `VICTORY_PACE_FPS`, ScarlettPort's victory) |
| `NEONBRIDGE_VSYNC=1` | Vsync for xcl (sets `XCL_PRESENT_SYNC=1`) |
| `NEONBRIDGE_BUILD=0` | Never build; use the exe as it is. |
| `NEONBRIDGE_BUILD_STAMP=0` | Use the mtime check instead of build stamps. |
| `NEONBRIDGE_PATCHES=0` | Build the trees without `vendor/patches`. |
| `NEONBRIDGE_WARM=0` | Skip the shader warm before a run. |
| `NEONBRIDGE_PRECOMPILE_GRAPHICS=0` | Skip ScarlettPort's graphics precompile (`NEONBRIDGE_GRAPHICS_IDLE_S`, default 60, is its idle limit per chunk). |
| `NEONBRIDGE_SHADER_PRUNE=0` | Keep every shader-cache toolchain folder (default: the 4 newest). |
| `NEONBRIDGE_SHADER_ROOT` | The shader cache folder (default `%LOCALAPPDATA%\6ixpack\cache\neonbridge-own`; `NEONBRIDGE_SHADER_ROOT_MOVE=0` keeps the old `%TEMP%` one). |
| `NEONBRIDGE_BUILD_VCVARS=0` | Run a Python build without the x64 Visual Studio environment. |
| `NEONBRIDGE_AGILITY_DIR` | The D3D12 Agility SDK DLLs (default `%LOCALAPPDATA%\6ixpack\agility-1.614.1`, from the official Microsoft.Direct3D.D3D12 1.614.1 NuGet package). |
| `NEONBRIDGE_FPS=1` | Start with the fps overlay shown. |
| `NEONBRIDGE_SPLASH=0` | No loader window. |
| `NEONBRIDGE_CACHE` | Another root for the runtimes' caches. |

## Files

| What | Where |
|---|---|
| Logs | `%LOCALAPPDATA%\6ixpack\logs\<runtime>_<time>.log`, the last one also in `%LOCALAPPDATA%\6ixpack\<runtime>\cache\last-run.log` |
| Saves | `%LOCALAPPDATA%\6ixpack\save` (the victory family shares it) |
| Caches | `%LOCALAPPDATA%\6ixpack\cache` (shared by the victory family), `%LOCALAPPDATA%\6ixpack\<runtime>\cache` (others) |
| Picker memory | `%LOCALAPPDATA%\6ixpack\last_game.txt`, `last_pick.txt` |
| Mixes | `%LOCALAPPDATA%\6ixpack\mix\<hash>` |
