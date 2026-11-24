# 6ixlaunch: run Xbox Series games on a Windows PC

![6ixlaunch](assets/banner.svg)

**In plain terms:** Xbox Series consoles and Windows PCs share the same kind of processor. Their games differ mainly in the system pieces they call into, like graphics, files, input and sound. 6ixlaunch lets a PC start a game copied from your own Xbox. It swaps those console pieces for Windows versions, so the game runs directly, with no slow emulation of a whole console.

It's an early work in progress: the GTA VI Xbox build starts and draws frames, but the screen is still black, it currently stops on an assert a few seconds in, and it isn't playable yet (see [Status](#status) and [Roadmap](#roadmap)). You need your own legally dumped copy of a game. None is included.

**Technical summary:** runs a dumped Xbox Series (Scarlett / GDK) executable on Windows. The game's x64 code runs natively; its console DLLs are replaced by TuxBox Rust providers built from `vendor/tuxbox` (source only, no binaries).

```
6ixlaunch --game <dump dir> [--sandbox] [--debug | --cdb <commands file>] [--log <file>]
6ixlaunch --game <dump dir> --restore
```

Needs Visual Studio Build Tools (x64 C++), cargo, and the x86_64-pc-windows-msvc target. `--sandbox` needs Sandboxie-Plus. `--debug` needs cdb from the Windows SDK. Run `6ixlaunch --help` for every option.

## Status

The Xbox Series build of GTA VI (`game_scarlett_master.exe`, from your own dump) gets this far:

- Runs natively on Windows with the console DLLs replaced by Rust providers.
- Passes `xgameruntime` initialisation, maps the console `G:` drive to the executable's folder and mounts every `.rpf` archive.
- Creates a D3D12 device and presents frames continuously, with no device removal. Runs have passed 900 frames at about 5 fps after a slow load.

Runtime fixes in the vendored TuxBox provider:

- **Black-frame causes:**
  - A stale inline descriptor binds a typed null instead of failing the command list.
  - A failed batch drops only itself, instead of blocking every later submission.
  - A GPU memory wait after rendering commands is skipped, instead of rejecting the list.
  - Placed textures over unmapped pages upload zero-filled, instead of never refreshing.
- **Presentation:** the swap chain resizes when the title changes its output size or format.
- **Command allocators:** a list reset on an allocator another list is still recording from gets an extra host allocator (Xbox allows this; desktop D3D12 returns E_INVALIDARG).
- **Views D3D12 rejected:**
  - Null UAVs on a depth image's stencil plane drop the plane slice.
  - B5G5R5A1 UAVs are viewed as R16_UINT.
  - R32_UINT render targets are castable to R9G9B9E5.
- **Memory D3D12 can't import:** buffers and placed buffers that span separately mapped title pages use device-local copies.
- **Shader compiler:** the launcher stages the SDK's `dxcompiler.dll` and `dxil.dll` beside the game for rectangle-list pipelines.
- **Diagnostics:**
  - A PresentX heartbeat every 300 frames.
  - The log names the view call that removed the device.
  - `--debug` no longer logs the title's illegal-instruction CPU probes.

## Open

- **Startup assert:** with `TUXBOX_PREWARM=0` the title stops on its own `int 3` about 4 s in, after its first frame: a command-list Reset returns E_INVALIDARG.
- **Shader translation:** offline `--build-shaders` refuses most programs, mainly descriptors without the title's root layout. Refusal reasons are now kept in `.unsupported` and tallied.
- **No window on screen:** the game presents, but no window appears on the desktop.
- **Early exit:** some runs exit with code 1 after about 40 s, without an exception.
- **B5G5R5A1 UAVs:** the shader writes float colour into an R16_UINT view, so the 5551 packing is wrong.
- **Memory layout:** title memory is mapped as separate 64 KiB section views. D3D12 can't open one heap across them, so large buffers fall back to per-submission copies. Backing each pool with one section would remove the copies.
- **Shader prewarm:** a full prewarm (`TUXBOX_PREWARM`, on by default) waits on remaining translator refusals in `xshader`. Use `TUXBOX_PREWARM=0` for quick runs.

## Roadmap

1. **Get past the startup assert, then see the frames.** Find why the presentation window isn't on screen, then capture a frame to check what the title draws.
2. **Stay running.** Find what calls process exit in the early-exit runs (`--debug` records the exit stack).
3. **Correct colour.** Pack 5551 in the shader, or substitute B5G5R5A1 UAV and render-target images with R8G8B8A8.
4. **Speed.** Back each XMem pool with one section and join in-order page runs, so GPU imports span them without copies.
5. **Shaders.** Finish the remaining `xshader` refusals (dynamic `s_load` offsets, mesh shaders), then run a full prewarm and ship the pipeline cache.
6. **Input and audio.** Check GameInput and the audio path once the title reaches its menu.
7. **Playable.** Reach the menu, load into the world, profile frame time.

No game files, keys or binaries are in this repository. Bring your own copy of the game. Not affiliated with Rockstar, Take-Two or Microsoft.

## Licence

`vendor/tuxbox` is TuxBox, under Apache-2.0 (see `vendor/tuxbox/LICENSE`, and the notices under `vendor/tuxbox/docs/licenses`). This repository's own code has no licence file yet.
