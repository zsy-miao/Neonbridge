# 6ixlaunch

Runs a dumped Xbox Series (Scarlett / GDK) executable on Windows. The game's x64 code runs natively; its console DLLs are replaced by TuxBox Rust providers built from `vendor/tuxbox` (source only, no binaries).

```
6ixlaunch --game <dump dir> [--sandbox] [--debug | --cdb <commands file>] [--log <file>]
6ixlaunch --game <dump dir> --restore
```

Needs Visual Studio Build Tools (x64 C++), cargo, and the x86_64-pc-windows-msvc target. `--sandbox` needs Sandboxie-Plus. `--debug` needs cdb from the Windows SDK. Run `6ixlaunch --help` for every option.

## Status

The Xbox Series build of GTA VI (`game_scarlett_master.exe`, from your own dump) gets this far:

- Runs natively on Windows with the console DLLs replaced by Rust providers.
- Passes `xgameruntime` initialisation.
- Maps the console `G:` drive to the executable's folder.
- Mounts every `.rpf` archive (common, movies, shaders, anim, data, levels, navmeshes, packs, textures, audio, script).
- Creates a D3D12 device and a window. The window stays white.

## Where we are stuck

The run stops on a D3D12 `OpenExistingHeapFromAddress` failure when the title's CPU (xmem) memory is imported as a GPU heap. Nothing is drawn after that.

Suspects, not yet confirmed: the call may be a console device slot forwarded to the PC device by number (console slot 48 creates a command queue, while PC slot 48 is `OpenExistingHeapFromAddress`), or the range may break D3D12's import rules (fully committed, one allocation, at most 4095 MB at a 4 GiB-aligned address). Other projects get past this by importing per-page slices, or by keeping host-side shadow copies of title memory and copying only the pages the title wrote.

No game files, keys or binaries are in this repository. Bring your own copy of the game. Not affiliated with Rockstar, Take-Two or Microsoft.

## Licence

`vendor/tuxbox` is TuxBox, under Apache-2.0 (see `vendor/tuxbox/LICENSE`, and the notices under `vendor/tuxbox/docs/licenses`). This repository's own code has no licence file yet.
