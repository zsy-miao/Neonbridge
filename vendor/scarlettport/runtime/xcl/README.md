# XCL – Xbox Series X|S → Windows compatibility layer

[![Discord](https://img.shields.io/badge/Discord-join%20the%20server-5865F2?logo=discord&logoColor=white)](https://discord.gg/xcl)
**Discord: <https://discord.gg/xcl>**

XCL is a compatibility layer that runs Xbox Series X|S (GDK) games you own on a Windows PC - in the spirit of Wine/Proton. There is no CPU emulation: the game's x64 code runs natively, and XCL provides its own, independently written implementations of the console system libraries the PC does not have (`kernelx.dll`, XGameRuntime, `d3d12_xs.dll`, …). The console's GPU code (RDNA2 machine code) is recompiled into PC shaders (HLSL → DXIL).

XCL works only on unpacked game files you already own. It does not download, decrypt or unpack game packages (XVC / MSIXVC) and contains nothing for circumventing DRM. The repository holds no code, headers or binaries from the console SDK, no keys and no game data; the Windows components XCL relies on (XGameRuntime, XAudio2, D3D12, DXC) come from your own system or the public Windows SDK and are not distributed with XCL.

## Games

| Game | Package (version) | State | What works | Limits |
|---|---|---|---|---|
| **Minecraft Preview** | `Microsoft.MinecraftBeta` 1.18.3064.0 | ✅ | fully playable | – |
| **Unicorn Overlord** | `SEGAofAmericaInc.Pa253b0c1Y` 1.0.0.6 | ✅ | fully playable | – |
| **Fortnite** | `436609B6.FortniteClient` 2.0.1461.7919 | ✅ | works fully, as far as the game allows without its online services | the game itself is online-only (multiplayer servers) |
| **XDefiant** | `Ubisoft.XDefiant-Game` 1.0.0.186 | ✅ | works fully, as far as the game allows without its servers | the game's own servers have been shut down |
| **A newer retail game** | – | 🚧 | starts in ~4 s, runs at a steady 30 fps without a crash, shows its menus and goes on past the first one to its settings; fits a 32 GB machine (~15 GB committed in the menus) | gameplay not reached yet; its minimap mesh pipelines are refused |

## Highlights

- **100 % of a retail game's shaders translate** – all 26 698 RDNA2 programs (26 503 records) of its shader pack
  become HLSL; random samples compile to DXIL with 0 errors.
- **The game's own console DLLs run on the PC** – its Bink 2 video codec decodes the game's
  movies, its ray-tracing BVH builder builds acceleration structures on a PC GPU, the AMD FSR 4
  console upscaler runs a whole frame on a PC GPU, the GDK HTTP library initialises.
- **Console textures decode exactly** – the GFX10 tiling and mip layout reproduce 909 of 912
  textures of the game's texture archive byte for byte, its volume textures included.
- **370+ automated tests**, including GPU tests that compare translated shaders against expected
  results instruction by instruction.

## What works

✅ done and verified

### Running the game's code

| Feature | State | What it does | Verified by |
|---|---|---|---|
| **PE loader** | ✅ | relocations, imports by name / ordinal / delay-load, TLS callbacks, exception tables, game DLLs, data-driven import routing (HLE / Windows / stub) | the game's 16 console DLLs load with 0 unresolved imports |
| **kernelx** | ✅ | title memory (budget from MicrosoftGame.Config, physical pages), threads on the PC's processors, modules, OS / console info | unit + end-to-end tests |
| **Memory** | ✅ | title graphics memory shared by CPU and GPU without copies, up to 16 GB (4 GiB windows) | GPU tests: loads, stores and atomics across windows |
| **File system** | ✅ | console paths (`GAME:`, `G:`, `D:` …) → the game folder; writes go to an overlay, the game folder is never modified; the console's drives: `S:\` (persistent local storage) and `T:\` (temp) in the title's data folder; free disk space answered for console paths | end-to-end: the test title's folder stays byte-identical; PLS and free space through the real call path |
| **DirectStorage** | ✅ | the console's DirectStorage queues: file and memory sources, zlib, and BCPack – the console's texture compression – for BC1, BC3, BC4, BC5, BC6H and BC7 (BC2 and the hardware swizzle are refused) | unit tests on synthetic streams; retail BCPack chunks of all six modes byte-identical with a reference decoder (`XCL_SAMPLE_BCPACK`) |

For a DirectStorage access-failure investigation, `XCL_DSTORAGE_FAILURE_DETAILS=1` adds
bounded failure records (up to 8 per queue, 64 per process): the failing stage, whether
the batch came from half-full auto-submission or the title's `Submit`, and counts of
zero page-array entries. Added records contain no addresses or file contents and do
not retry, replace, or reschedule requests. Off by default.

### System services

| Feature | State | What it does | Verified by |
|---|---|---|---|
| **XGameRuntime** | ✅ | the GDK thunk protocol real titles use, laid out from a table of all 396 GDK runtime functions; offline XUser, XGameSave, persistent local storage, XPackage, XGame (title id), XSystem – the title sees an Xbox Series X / S; task queues, async, networking and dialogs through the PC's XGameRuntime | the game's libHttpClient initialises; users, saves and storage through the real call path; slot layout cross-checked against the game's own thunks |
| **Input** | ✅ | GameInput: one gamepad – the PC's XInput controller, and keyboard and mouse as a gamepad (see [Controls](#controls)) | unit tests; an Xbox controller in Minecraft |
| **Audio** | ✅ | XAudio2 → system XAudio2.9 (PCM / float voices; Opus 0x704F decoded via `MSOpusDecoder.dll`; XMA1 / XMA2 decoded in pure Rust) | unit + end-to-end tests |

### Graphics

On wrapped command lists, an eligible `Wait64BitValueX` (`>=`) after GPU work splits the
recording so the prefix can run before the wait. Untracked counters still use a bounded CPU wait.

| Feature | State | What it does | Verified by |
|---|---|---|---|
| **Shader recompiler** | ✅ | RDNA2 → HLSL → DXIL: VS, PS, CS, mesh (NGG), GS; atomics, LDS, scratch, images, samplers incl. static ones, wave ops, ray-tracing instructions; structured control flow; parallel DXC, disk cache | 26 698 / 26 698 programs of the retail pack; 0 DXC errors on random samples; per-instruction GPU tests |
| **Pipelines** | ✅ | PSOs from console shaders: VS+PS, CS, MS+PS, VS+GS+PS; root signature with the shaders' static samplers; non-blocking creation – a pipeline whose shaders are not compiled yet gets a stand-in at once and is built in the background | GPU tests drawing the game's own VS / PS pairs, also built in the background |
| **D3D12.X compute** | ✅ | console device slots, console descriptor heaps (T# / V# / S#), buffers at their console addresses, compute command lists recorded in the console format and replayed (tables, barriers, clears, copies) | the game's BVH builder and FSR 4 record and run through XCL |
| **D3D12.X graphics** | ✅ | device, resources and heaps (also placed in title memory), pipelines; the title's graphics command lists – draws, indirect draws, graphics root arguments, barriers, copies, clears – and command lists recorded by console code, replayed; fence mirrors follow completed values even when a queue rewinds them; `PresentX` and frame pacing into an XCL window | Minecraft Preview plays its worlds (millions of draws, none skipped); Fortnite and XDefiant draw their screens |
| **Textures** | ✅ | GFX10 tiling (all 14 2D swizzle modes, the 6 3D thick modes, linear), mip chains with mip tail, arrays, cubes and 3D textures → PC textures | 909 / 912 retail textures exact (the 3 volume LUTs decode smooth); GPU upload bit exact |
| **Ray tracing** | ✅ | BLAS / TLAS built on the GPU by the game's own XboxBvhBuilder; RDNA2 BVH node format; BVH intersection instructions emulated in shaders | GPU traversal = brute force reference; the game's builder output checked on the GPU |
| **Upscaling** | ✅ | the game's AMD FSR 4 console DLL (`amd_fidelityfx_upscaler_xbox.dll`): context, the 29 pipelines in the console driver's own container (FSPX) translated, a frame's 20 passes recorded and run on the GPU | end-to-end with the real DLL: a 1280×720 → 1920×1080 frame on an RX 9070, the input's gradient in the output |

Immediate `Write64BitValueBottomOfPipeX` values in CPU-only memory are published atomically only
after the producer fence completes. A late producer does not block CPU writes from other queues.

### Media and tools

| Feature | State | What it does | Verified by |
|---|---|---|---|
| **Video** | ✅ | the game's Bink 2 console DLL decodes its movies (720p / 1080p) on the CPU; the game's own YUV → RGB display shaders (`PS_Bink…` in its pack) translate like any other. Bink's GPU-assisted mode needs no Bink-specific support: its compute shaders ship with the game and go through the recompiler too (no game seen using it yet) | a game movie decodes frame by frame, picture checked |
| **Tools** | ✅ | `xcl inspect`, `xcl run`, `xcl disasm` (RDNA2, console shader containers), `xcl shaders` (pack statistics, recompile / compile runs), `xcl rpf`, `xcl pipelines` (compiles a run's pipelines outside the game) | – |

## Controls

A **gamepad is recommended**: console games are made for one, and their menus and controls expect it.

- **Gamepad:** any controller Windows sees through XInput (Xbox controllers, wired or wireless) is the
  game's gamepad – buttons, sticks, triggers and rumble. It can be plugged in or out while the game runs.
- **Keyboard and mouse** work too, as a gamepad. Click into the window to capture the mouse (cursor
  hidden and held in the window, raw mouse movement turns the camera); Alt, Alt+Tab or losing the focus
  release it, another click captures it again (the title bar says "click to capture mouse" while it is
  free). A deflected controller stick wins over WASD / the mouse.

  | PC | gamepad |
  |---|---|
  | W A S D | left stick |
  | mouse movement | right stick (camera) |
  | left / right mouse button | RT (shoot) / LT (aim) |
  | wheel up / down | D-pad up / down |
  | middle mouse button | right stick click |
  | Shift | A (sprint) |
  | Space | X (jump) |
  | E, F | Y (vehicle) |
  | Q / R | LB / RB |
  | Ctrl | left stick click |
  | Enter | A |
  | Escape, Backspace | B |
  | arrows / Tab / P | D-pad / View / Menu |

  `keyboard_mouse` and `mouse_sensitivity` in the game's `xcl.toml` turn it off or change the mouse speed;
  `XCL_MOUSE_SENS=<f>` and `XCL_MOUSE_INVERT_Y=1` override it per run. `XCL_KEYMAP=<path>` rebinds keys
  with a text file of `KEY=BUTTON` lines (`#` comments; keys A-Z, 0-9, F1-F12, SPACE, ENTER, ESC,
  BACKSPACE, TAB, SHIFT, CTRL, arrows; buttons A B X Y LB RB LT RT LS RS VIEW MENU UP DOWN LEFT RIGHT,
  NONE unbinds), e.g. `SPACE=A`.

## Display

Environment variables (XCL keeps no settings of its own; a launcher passes them). The title renders at its
own resolution; the image is scaled into the window, letterboxed, with a Catmull-Rom filter.

- `XCL_WINDOW_MODE`: `windowed` | `borderless` | `fullscreen` (exclusive). Alt+Enter toggles windowed / borderless.
- `XCL_BACKGROUND`: `mute` (default) | `pause` | `run`. Window out of focus: mute the audio and report the title constrained (as the guide does), also stop the audio engines, or change nothing.
- Crash reports: an unhandled exception, a non-zero exit or a device removal writes `%LOCALAPPDATA%\xcl\crash-reports\<time>.zip` (last 5000 log lines, `XCL_*` variables, GPU/driver, OS build, XCL build; the user name is scrubbed from paths).
- `XCL_WINDOW_SIZE`: `WxH`, the window's client size (windowed) or the display mode (fullscreen).
- `XCL_FPS_CAP`: `30` | `60` | `0`. A cap lengthens the title's frame interval; `0` adds none.
- `XCL_VSYNC`: `1` presents at interval 1; `0` at interval 0 with tearing where supported.
- `XCL_MONITOR`: monitor index (`EnumDisplayMonitors` order).

## Building and usage

Windows 10/11 x64, Rust (`stable-x86_64-pc-windows-msvc`), the Windows SDK (DXC: `dxcompiler.dll`
+ `dxil.dll`, looked up in `Windows Kits\10\bin\<version>\x64`, next to `xcl.exe` or in
`%XCL_DXC_DIR%`), a D3D12 GPU with Shader Model 6.6 and mesh shaders, and the system XGameRuntime
(`System32\xgameruntime.dll`, part of Windows / Gaming Services). On **Windows 10** (22H2
KB5029331+), inbox `D3D12Core.dll` stops at SM 6.5; place the Microsoft D3D12 Agility SDK
(`D3D12Core.dll`, plus `d3d12SDKLayers.dll` if using `XCL_D3D12_DEBUG=1`) in a `D3D12\` folder
next to `xcl.exe`, directly next to `xcl.exe`, in `%LOCALAPPDATA%\XCL\D3D12\`, or point `%XCL_D3D12_DIR%` at it
(for `cargo test`, use `%LOCALAPPDATA%\XCL\D3D12\` or `%XCL_D3D12_DIR%`).

- **DXC:** 1.8 or newer is recommended, from the
  [DirectXShaderCompiler releases](https://github.com/microsoft/DirectXShaderCompiler/releases): unpack one
  and point `XCL_DXC_DIR` at its `bin\x64`. A Windows SDK may hold an older one (the same `10.0.26100.0`
  folder has DXC 1.7.2308 or 1.8.2502, depending on the SDK's revision). DXC 1.7's DXIL was 2–2.7 times the
  size of 1.9's for translated compute programs and took an NVIDIA driver 1.4–2.2 times as long to build
  pipelines from ([#156](https://github.com/ivall/xcl/issues/156)). `xcl run` prints the DXC it loaded, and
  `report.json` has it under `dxc`.
- **GPU:** developed on AMD RDNA4. The console's geometry and NGG programs run as a compute pass plus a
  mesh pass, so a GPU without mesh shaders draws none of them. On GPUs with 32-lane waves only
  (NVIDIA) console wave64 compute, geometry and mesh programs run as two 32-lane halves that exchange
  masks and lane values ([`docs/wave64-on-wave32.md`](docs/wave64-on-wave32.md)) – slower than
  64-lane waves, most for groups of several waves; wave64 vertex and pixel shaders that read lanes 32–63
  can still differ there. Test results on other GPUs: [#42](https://github.com/ivall/xcl/issues/42).

```powershell
cargo build --release --workspace          # target\release\xcl.exe

xcl inspect D:\Game\game.exe               # the game's imports and what XCL has for them
xcl run D:\Game [-- game arguments]        # run
xcl disasm <shader.bin> [--hex]            # RDNA2 disassembler (console containers too)
xcl shaders D:\data\shaders.rpf --top 50   # pack statistics; --recomp / --compile N
xcl shaders D:\data\shaders.rpf --precompile --only cs --pipelines   # build compute pipelines and save driver blobs for the next run
xcl rpf D:\data\shaders.rpf --extract NAME --out folder
xcl pipelines D:\Games\SomeTitle [--watch]  # compile the pipelines a run queued (<data>\cache\pipelines.bin)
```

- **Without a command line** (xcl.exe opened from Explorer) XCL shows a small menu: *Choose a
  game…* (pick its exe or `MicrosoftGame.config`) or *Run again* (the last one). On an error the
  window stays open until Enter.
- XCL works on **unpacked** game files you own; it contains nothing for decrypting or
  circumventing DRM.
- **The game folder is never modified** – writes go to the overlay.
- Translated shaders are reused across XCL rebuilds when the translator sources and DXC
  toolchain are unchanged; `XCL_TRANSLATED_CACHE=0` disables that disk cache.
- Every run writes `report.json` (called stubs, unknown D3D12.X slots and interfaces, XGameRuntime
  classes asked for, shader statistics, audio formats, crash).
  `d3d12.presents` counts presentations, not visible content. With `XCL_FRAME_DUMP=N` (`N > 1`),
  the existing optional frame dumps also set `d3d12.last_frame_dump`: the latest sampled frame's
  zero-based index, source dimensions, DXGI format, sample step, sample count and `non_dark_samples`
  (decoded 8-bit R+G+B > 30). The field is `null` until a frame is sampled; `non_dark_samples` is
  `null` if its format cannot be decoded. Intermediate render-target dumps do not update it.
  Sampling uses every fourth pixel in each direction (`XCL_FRAME_DUMP_FULL=1`: every pixel).
  A coarse sample can miss detail; a non-dark sample does not establish correct rendering or gameplay.
- Title data: `%LOCALAPPDATA%\XCL\<title>\` (`%XCL_DATA_ROOT%`): `xcl.toml`, `overlay\`, `saves\`,
  `pls\` (the title's `S:\`), `temp\` (`T:\`), `cache\`, `logs\xcl.log`, `logs\report.json` (the
  previous run's: `xcl.prev.log`, `report.prev.json`).

## Tests

```powershell
.\scripts\test.ps1                         # cargo build --workspace + cargo test --workspace
```

Unit tests, an end-to-end test with our own "console title" (`tests/testtitle`) and GPU tests
(D3D12, SM 6.6; skipped without a GPU / DXC; `XCL_ADAPTER=1` or `XCL_ADAPTER=radeon` picks
another adapter than the first high-performance one, for titles too). A GPU test that cannot run on
the adapter (no GPU / DXC / wave64 / mesh shaders) passes too, but prints `SKIPPED <test>: <reason>`, and
`scripts/test.ps1` counts them at the end; `XCL_REQUIRE_GPU=wave64` (or `mesh`, or `wave64,mesh`) makes
those skips fail. Tests on real game data are opt-in through
environment variables pointing at your own files – `XCL_SAMPLE_RPF` (shader pack),
`XCL_SAMPLE_DLLS` (the game's console DLLs), `XCL_SAMPLE_TTPF` (texture archive),
`XCL_SAMPLE_BCPACK` (a folder of BCPack streams `<name>.bc<mode>` with a reference decoder's blocks of each, `<name>.vic<mode>`), `XCL_SAMPLE_BK2` (a movie), `XCL_SAMPLE_BVH4`, `XCL_SAMPLE_AWC` (a sound) – described in `HANDOFF.md`; without them those
tests pass without running anything (`scripts/test.ps1` lists the ones not set). No game data is
part of the repository.

## Licenses

**GPL-2.0-only** (`LICENSE`); third-party sources: `THIRD_PARTY.md` (including AMD's
machine-readable ISA, AMD addrlib swizzle patterns, LLVM disassembler tests, AnyPS5,
libHttpClient headers). WinDurango serves only as a reference – no code is ported from it. The
repository contains no headers or binaries from the console SDK.
