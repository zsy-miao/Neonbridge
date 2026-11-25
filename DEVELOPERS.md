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

| `--runtime` | What it is | Upstream |
|---|---|---|
| `scarlettport` (default) | ScarlettPort's VIctory-based runtime and launcher scripts | ZigZagBoi1/ScarlettPort |
| `victory` | VIctory-fxceless, the victory emulator fork | fxceless/victory-fork |
| `vitoriabr` | VItoriaBr's VIctory fork | uiliannn/VItoriaBr |
| `ztrx-victory` | ztrx's VIctory fork | Zettrox/ztrx |
| `xcl` | xcl | ivall/xcl |
| `tuxbox` | brcly's TuxBox, upstream as vendored, run through its Python CLI | brcly/TuxBox |
| `fivestar` | Five Star Recomp (.NET 10, PC GDK) | ThatRecompDev |
| `neonbridge-own` | Neonbridge (own build): TuxBox console-library providers staged into the game folder, with Neonbridge's fixes and shader translator | brcly/TuxBox |

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
| `NEONBRIDGE_LOG=0` | No run log (logging is on by default; children get `RUST_LOG=info`, the victory family also `VICTORY_LOG=info`). |
| `NEONBRIDGE_DIAG=1` | Diagnostics build (`--features diagnostics` in `target-diag`) and `VICTORY_D3D12_DRED=1`. The picker's Diagnostics box sets it. |
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
