# Platforms and performance

## Why C# is fast enough

The console's CPU is x86-64, so GTA VI's own code runs as native machine code at full speed. C# only hosts it: the
loader, the console runtime and the graphics translation layer. What has to stay fast are the calls from the game
into that layer, thousands per frame.

| Rule | Purpose |
|---|---|
| NativeAOT builds | no JIT warm-up, native startup |
| Entry points are function pointers (`UnmanagedCallersOnly`) | a game-to-shim call costs about as much as a native call |
| `[HotPath]` methods allocate nothing and take no per-call locks | no garbage collection during a frame; enforced by the code-rule tests |
| Benchmarks for call and command-translation overhead | regressions show up as numbers, not as stutter |

## GPU performance

| Item | Approach |
|---|---|
| Shader control flow | translated programs use structured `if`/`else` and loops, close to the original: DXIL 39% smaller and dxc 16% faster than with the older dispatch loop |
| Shader compilation | every program is compiled ahead of time and shipped with a pipeline cache, so nothing compiles during play |
| Memory | the console's unified, directly addressed GPU memory maps onto D3D12 heaps; designed once the executable shows how the game uses it |

## Platforms

| Platform | Route |
|---|---|
| Windows | primary target; D3D12 is the closest match to the console's D3D12.X. The game uses four instruction families only AMD CPUs have (SSE4a, CLZERO, MONITORX/MWAITX, CLWB); on Intel CPUs the loader emulates them ([cpu.md](https://github.com/ThatRecompDev/five-star-recomp/blob/main/docs/cpu.md)), checked so far only through a forced self-test on AMD |
| Linux and Steam Deck | the Windows build through Proton (Wine with vkd3d-proton); tested as a supported target |
| Linux, native | later: a runtime for the Windows parts the game uses and thunks between the Windows and Linux x64 calling conventions |
| macOS | the Windows build through CrossOver or Apple's Game Porting Toolkit (Rosetta 2 for x86-64, D3D12 on Metal); a native port would need x86-64 to ARM64 recompilation and is out of scope |

## Wine and Proton check

Not run yet: no machine with Wine was available. Linux .NET already builds the solution and passes every test except
the loader's, which call kernel32 directly. The check of the Windows build under Wine:

```sh
dotnet publish src/FiveStar.Cli -c Release -r win-x64 --self-contained -o out/cli
dotnet publish src/FiveStar.Loader -c Release -r win-x64 --self-contained -o out/loader
dump=<your dump folder>
wine out/cli/fivestar.exe --help
wine out/cli/fivestar.exe texture extract "$dump/<file>.ttpf" out/textures
wine out/loader/fivestar-loader.exe --load "$dump/XboxBvhBuilder.dll" --load "$dump/Bink2Scarlett.dll" \
  --load "$dump/amd_fidelityfx_loader_xbox.dll" --load "$dump/amd_fidelityfx_upscaler_xbox.dll" \
  --load "$dump/libHttpClient.GDK.dll" --load "$dump/XCurl.dll" --call all --unload --log run.log
```

Under Proton, run the same executables with `proton run`, with `STEAM_COMPAT_DATA_PATH` set to an empty prefix folder
and `STEAM_COMPAT_CLIENT_INSTALL_PATH` to the Steam folder.
The loader result to compare with Windows is the import summary (825 imports, none unresolved) and the export calls.

GPUs with 32-lane waves (NVIDIA, Apple) run the emulated wave; 446 pixel and vertex programs, 37 mesh programs and 19 compute
programs read lanes a 32-lane wave does not have; see [Shader translation](Shader-Translation.md#wave-size).
