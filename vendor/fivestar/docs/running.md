# Running the game

`fivestar run` takes your own console dump to a running game: it checks the PC, builds what the loader needs in a
work folder (once; later runs reuse it), then starts `fivestar-loader` on the game executable.

## Shortest path

1. Install the .NET 10 SDK, the Windows SDK (for `dxc`) and the [Microsoft GDK](https://github.com/microsoft/GDK)
   (the PC GDK; its `xgameruntime.lib` gives the loader the GDK's call table).
2. Build: `dotnet build FiveStar.slnx`.
3. Check: `fivestar doctor <dump folder>`. Every line should say `ok`; a `FAIL` line says what to install or pass.
4. Run: `fivestar run <dump folder>`.

The first run extracts and translates every shader program (25,908 programs, about an hour on 24 cores, 16 GB of
disk) before the game starts; it can be stopped and resumed. Later runs start the game in under a second.

The game uses four AMD-only instruction families (SSE4a, CLZERO, MONITORX/MWAITX, CLWB). On an Intel CPU the loader
emulates them in its exception handler and logs a `cpu:` line for each place it does ([cpu.md](cpu.md)).

`fivestar` is `artifacts/bin/FiveStar.Cli/debug/fivestar.exe` after the build (or `dotnet run --project
src/FiveStar.Cli --`).

## Command

```
fivestar run <dump folder> [--work <dir>] [--debug] [--timeout <s>] [-- <loader options>]
fivestar doctor <dump folder> [same options]
```

| Option | Meaning |
|---|---|
| `--work <dir>` | work folder; default `%LOCALAPPDATA%\FiveStar\<titleid>-<version>`, the same as `fivestar setup` |
| `--debug` | the D3D12 debug layer and DRED (`fivestar-loader --d3d12-debug`); needs the Graphics Tools optional feature |
| `--timeout <s>` | end the game after s seconds (`--run-timeout`, exit code 3) |
| `--shaders <dir> --translated <dir>` | use existing extracted programs and translated DXIL instead of building them; the translated folder needs `rect-gs/` |
| `--loader <exe>` | `fivestar-loader.exe`; default `FIVESTAR_LOADER`, then the folder of `fivestar`, then the build tree |
| `--dxc`, `--gdk-lib`, `--bcpack-tables`, `--jobs` | as for [setup](setup.md) |
| `-- <options>` | passed to `fivestar-loader` after its own options ([loader.md](loader.md#running-it)) |

`run` returns the loader's exit code; 1 when a check or a setup step failed, 2 for a wrong command line. `doctor`
returns 1 when a check failed.

## What run does

```mermaid
flowchart LR
  checks["checks"] --> setup["setup steps not done yet"] --> loader["fivestar-loader --run"] --> summary["summary"]
```

1. **Checks** (also all `doctor` does, followed by the state of the work folder):

   | Check | Fails when |
   |---|---|
   | `windows` | not Windows x64 |
   | `dump` | no `MicrosoftGame.Config`, another title id, no `game_scarlett_master.exe` or `shaders.rpf`; another version warns |
   | `loader` | `fivestar-loader.exe` not found |
   | `gdk` | no `--gdk-lib`, no PC GDK under `Program Files (x86)\Microsoft GDK`, no table from an earlier run and no `--gdk-abi` after `--` |
   | `bcpack` | warns when the work folder has no BCPack tables: BC3, BC6H and BC7 pages then get placeholder blocks |
   | `dxc` | dxc (and the `dxcompiler.dll` beside it, which the loader compiles programs with on first use) not found, unless `--shaders`/`--translated` give built shaders |
   | `disk` | the drive of the work folder has less free space than the steps still to do need, plus 2 GiB for the run |
   | `shaders` | with `--shaders`/`--translated`: a folder or `rect-gs/` missing; no `variants.json` warns |
   | `debug layer` | `--debug` and no `d3d12SDKLayers.dll` in System32 |

2. **Setup steps**, through the same runner and state as [`fivestar setup`](setup.md), so a step done by either
   command is done for both: `verify`, `shader-extract`, `bcpack-tables`, `gdk-abi`. The loader translates each
   program when the game first uses it ([d3d12x-pipelines.md](d3d12x-pipelines.md#programs-on-demand)); with
   `--translate-ahead` also `shader-translate` (all variants the loader picks, with `variants.json`) and `rect-gs`
   first. With `--shaders`/`--translated` only `verify`, `bcpack-tables` and `gdk-abi`.
3. **Loader**, started as

   ```
   fivestar-loader --load <dump>\game_scarlett_master.exe --no-exports --work-dir <work>\loader\tls
     --log <work>\logs\run-<time>.log --run [--run-timeout s] --shader-dir <work>\shaders\fssm
     --program-cache <work>\program-cache [--translated-dir <work>\shaders\hlsl] [--dxc <dxc>]
     --pipeline-cache <work>\pipeline-cache [--gdk-abi <work>\loader\gdk-abi.txt]
     [--bcpack-tables <work>\loader\bcpack-tables.bin] [--d3d12-debug] [<loader options>]
   ```

   `--translated-dir` is passed with `--translate-ahead` or `--translated`, `--dxc` when run gets it.

   The console shows the loader's milestones (`==` lines, errors, faults with their address, presents); the full log
   is the file. `--pipeline-cache` is left out when the loader options name `--pipeline-cache` or
   `--no-pipeline-cache`. At the end run prints a summary from the loader's lines, for example
   `fivestar-loader exit code 3: 293 presents, 0 faults, 43 warnings; full log ...`. Ctrl+C reaches the loader as well;
   run waits for it to end before the summary.

## First run in the game

With an empty save folder the game shows a black screen that waits for a press (with a warm pipeline cache 45 to
220 s after start), the logos and the intro art, then: a minimap choice, a text-size choice, subtitles
(Off by default, Left and Right change it), brightness calibration, a controls notice, the legal and photosensitivity
screens, and a privacy policy and terms of service screen, before it loads story mode (a spinner on dark blue). Keys of the
keyboard-and-mouse gamepad: A (Space) confirms, a short press too; arrows move the focus. The photosensitivity item
keeps the focus for 4 s. The two terms items are check boxes that A toggles (scrolling their text with the right
stick, the mouse, is not required); with both checked Confirm is enabled. The first press after Next gets the focus on the legal screen can be dropped. While pipelines compile the game can miss a
0.2 s press, and an arrow held 0.5 s or longer repeats. With the input script followed while
running ([loader.md](loader.md#running-the-executable)) these can be answered from a second shell;
`-- --input-script docs/first-run.input` answers them unattended: it taps Space until the subtitles screen shows, turns
subtitles on, taps Space until the legal screen shows, then waits for each screen it needs rather than for a time. It
recognises screens it learned from your own frames (`%LOCALAPPDATA%\FiveStar\screens.txt`); the first time, press F8
when the subtitles screen, the legal screen, the terms screen and the story load show (and the legal screen once Next has the focus), and later runs need no help. Only the first run shows these
screens (below). B
(C) goes back to the previous screen. The screens' text, headings and logos render in full; the brightness-calibration
symbol is still missing (every pixel right of the panel is 0 at level 15). It is not the frame's first draw: that is
the executable's `VS_Clear` + `PS_Clear` ([shader-containers.md](shader-containers.md#executable)), which reads its
rectangle and colour through a root SRV: the console address goes to the program as passed and is read through the
address map (`RealClearRunner` runs the real program so on WARP). The next draw (`PS_BlitColour` with a zero colour) fills the target black, and the UI is drawn over it.

**Later boots skip the screens.** At the terms screen the game writes `Player` and `SettingsProfileSettings`
(settings and the first-run choices); the next boot goes from the logos straight to the story load. Start-up step 4 queues their loads one after the other on the same
save slot. Each starts with an XAsyncRun whose completion routine calls `XAsyncGetStatus(block, true)` on its own
block before it frees the block and the slot's in-use mark; the loader used to block there until the routine
returned, so the settings load found the slot in use and failed, and the screens came back.

## Work folder

| Path | Holds |
|---|---|
| `state/` | setup step records ([setup.md](setup.md#state-and-resume)) |
| `shaders/fssm`, `shaders/dxbc` | extracted programs and containers |
| `shaders/hlsl` | translated HLSL and DXIL, `variants.json`, `results.json`, `rect-gs/` |
| `shader-cache/` | translations and compiles by hash; a re-translate after an update reuses them |
| `loader/gdk-abi.txt`, `loader/tls/` | the GDK call table; the loader's TLS host DLLs |
| `pipeline-cache/` | the desktop pipelines built by runs and by the `pipeline-warm` setup step ([d3d12x-pipelines.md](d3d12x-pipelines.md#disk-cache)) |
| `logs/run-<yyyyMMdd-HHmmss>.log` | the full loader log of each run; the newest 5 are kept |

Saves go to the loader's default, `%LOCALAPPDATA%\FiveStar\GdkData\<TitleId>`. The default work folder is on the
system drive; pass `--work` to put the shaders' 16 GB elsewhere.
