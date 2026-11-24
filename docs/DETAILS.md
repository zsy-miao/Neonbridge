# Neonbridge: technical details

The [README](../README.md) has the short version. This page lists what works, every runtime fix, the diagnostic switches and the open issues.

## Status

What the Xbox Series build of GTA VI (`game_scarlett_master.exe`, from your own dump) does today:

- **Boot:** it runs natively with the console DLLs replaced by Rust providers, passes `xgameruntime` initialisation, maps the console `G:` drive to the executable's folder and mounts every `.rpf` archive.
- **Rendering:** it creates a D3D12 device and draws lit frames in a window fitted to one monitor at 16:9: the GTA stars intro, the player-selection prompt, then the menus beyond it. There's no white flash at start: the window stays hidden until its first frame.
- **Speed:** about 16 fps (it was 5 fps until the game's page-by-page memory was joined so the GPU uses it in place instead of copying ~1.2 GB every submission).
- **Input:** the title reads the virtual pad, and an automatic A press gets it past the player-selection prompt.
- **Saves:** XGameSave containers are stored under `%LOCALAPPDATA%\TuxBox\PersistentLocalStorage\<title id>\GameSave\`, and the game loads its "Player" save on the next launch.
- **Offline and contained:** the game's network calls fail as "no network", its `D:` drive is a local folder, and it runs in a job that ends with the launcher.
- **Audio:** sound plays but comes in 3 s bursts. The game's Opus stream is fed at about 15% of real time; the console DSP (`chad`) no longer refuses commands, which is the likely cause, and the next run will confirm it.
- **Menus:** dialogs, text entry, speech and relaunch requests are answered, so the menus no longer wait on missing console UI. The game now gets through the menus to the loading screen (the Leonida artwork); loading the world is next.
- **Longest run:** 344 s, ending on a fail-fast exit (`0xc0000602`). Crash records now name the import or function a crash went through.

## Runtime fixes

These are in the vendored TuxBox provider:

- **Present barrier:** the end-of-frame barrier that sends the display surface to present used layout 31, which desktop D3D12 rejects, so every frame's `Close` failed. Layouts outside the desktop range are now recorded as COMMON (= PRESENT). This is the fix that made frames appear.
- **Enhanced barriers (list slot 146):** texture barriers are fitted to the host resource (subresource range, layouts and accesses, DISCARD, buffer targets), and sync and access pairs are made legal.
- **Startup crash:** the title's list slot 131 call is a cache barrier, but it was routed to the ray-tracing build path, which read garbage memory. It now records a cache barrier. A host list whose `Close` fails is rebuilt instead of failing every later `Reset`.
- **Title asserts:** a release-build size check at game+0x4ffd879 hits `int 3` and then falls through, so the runtime steps over it when its bytes match (ported from upstream TuxBox).
- **Texture layouts:** Xbox layout 0x10a (64 KiB display swizzle, mipped) gets a real size and placement, instead of an invalid one that crashed the title.
- **Copies:** list slot 62 (`CopyMemoryToMemoryX`) records a buffer copy instead of failing fast.
- **Game saves (`xgameruntime`):** XGameSave containers, blobs, updates, quota and the save-files folder are stored on disk instead of returning nothing.
- **Unknown calls don't crash:** a D3D12.x slot we haven't mapped logs `unmapped slot <n>` and answers 0 instead of failing fast (`TUXBOX_UNMAPPED=failfast` restores the old behaviour). Newer list slots (fills, `DispatchX`, `CopyResourceX`, markers) and device slot 55 (derived pipelines) are handled, and GPU memory waits honour every comparison.
- **Platform:**
  - The title asks for a 1 ms (or finer) system timer, so its audio mixer isn't slowed by the default 15.6 ms tick (`TUXBOX_FINE_TIMER=0`).
  - The console model reads as Series X (`TUXBOX_CONSOLE`).
  - `clwb`, `rdpid` and `mcommit` are emulated on CPUs without them.
  - Title memory that the GPU window can't hold is placed below 2^40.
  - XAudio2 is created without console-only flags or core masks.
- **Textures:** BCPack-compressed textures get neutral grey blocks until a decoder exists (`TUXBOX_BCPACK=refuse`), and placed textures are only re-compared when the CPU wrote them.
- **Rendering:**
  - A stale inline descriptor binds a typed null instead of failing the command list.
  - A failed batch drops only itself, instead of blocking every later submission.
  - A GPU memory wait after rendering commands is skipped instead of rejecting the list.
  - Placed textures over unmapped pages upload zero-filled.
- **Presentation:** the swap chain resizes when the title changes its output size or format.
- **Root views:** root CBV/SRV/UAV binds import up to 64 KiB of committed title memory, and an uncommitted address binds null.
- **Queues and lists:**
  - Queue slot 29 (`CopyPageMappingsBatchX`) works on the older table.
  - Queries and split barriers are closed out before `Close`.
  - Copy lists get a timestamp heap.
  - Extended `Reset` options are ignored instead of refused.
- **Command allocators:** a list reset on an allocator that another list is still recording from gets an extra host allocator. Xbox allows this, but desktop D3D12 returns E_INVALIDARG.
- **Shader samplers:** a run-time descriptor table gives translated shaders host samplers for sampler words written into the shader code.
- **Views D3D12 rejected:**
  - Null UAVs on a depth image's stencil plane drop the plane slice.
  - B5G5R5A1 UAVs are viewed as R16_UINT.
  - R32_UINT render targets are castable to R9G9B9E5.
- **Memory D3D12 can't import:** buffers and placed buffers that span separately mapped title pages use device-local copies.
- **Shader translator (`xshader`):**
  - Descriptors without the title's root layout are bound from the shader's own FSSM and PSV records.
  - Compute-heap stores, wave width, VCC branch votes across the wave, `bpermute`, and many single instructions translate.
  - Hint and debug SOPPs are no-ops.
  - dxc retries at `-O0` when `-O1` times out.
  - `[WaveSize]` widths the host GPU lacks are compiled out (`TB_NO_WAVESIZE_<n>`), using the wave range the runtime reads from the device.
- **Speed:**
  - Buffer binds keep their imported memory alive, so recording dropped from about 156 ms to under 2 ms per frame.
  - The title maps memory one 64 KiB page at a time. Neighbouring pages are now joined into one view on demand, so large buffers import zero-copy instead of being copied in and out every submission.
  - Buffers are written back only after the GPU actually wrote them.
  - Per-frame constant buffers at new addresses reuse earlier imports and device copies instead of opening the title's memory again (`TUXBOX_SHADOW_IMPORT_CACHE=0`, `TUXBOX_SHADOW_COPY_REUSE=0`).
  - Page joins that cross two OS reservations are re-reserved as one block (`TUXBOX_JOIN_RERESERVE=0`).
- **GPU memory sizes:** allocation-info slots 25 and 108 answer the console's footprint for every texture and buffer, so GTA's 2.2 GB GPU pool doesn't fill early with PC sizes (the 87 s stop other projects hit). Committed textures' addresses are backed by real zeroed memory (`TUXBOX_BACK_IMAGES=0`).
- **Robustness:**
  - Draws whose descriptor table doesn't fit are dropped and counted instead of failing the list.
  - Unbacked vertex streams and bad address views bind empty; null vertex-buffer arrays unbind explicitly.
  - Allocators reset or released at a reused address start clean; recordings still in use survive a reset.
  - Reads past the end of a file fail cleanly (`TUXBOX_EOF_PARTIAL=1`).
- **Menus and runtime (`xgameruntime`):**
  - XGameUi: message dialogs answer their default button, text entry answers `TUXBOX_TEXT_ENTRY` or the default text, other dialogs cancel.
  - XAccessibility: a silent speech synthesizer (`TUXBOX_SPEECH=refuse`), captions and high contrast off.
  - `XLaunchNewGame` asks the launcher to relaunch (`TUXBOX_RELAUNCH=0`); a terminated task queue wakes its waiting dispatch.
  - Saves check the 256 MB quota and buffer sizes and return the console's error codes.
- **Audio:**
  - The title asks for a 1 ms (or finer) timer (`TUXBOX_FINE_TIMER=0`).
  - `chad` (XDsp) completes every command, with silence for transforms it can't compute (`TUXBOX_CHAD_STRICT=1`).
  - AcpHal answers connect, message queue and XMA submit, with an XMA decoder (LGPL `xma` crate, notice in `vendor/tuxbox/runtime/acphal/NOTICE`; `TUXBOX_XMA=0`).
- **Window and keys:** Alt and F10 no longer open the modal window menu that made the game abort (`TUXBOX_KEY_MENU=1`).
- **Shader translator:** many more instructions and fixes, among them `v_permlane16`, `v_mad_i64_i32`, `s_memtime`, `s_bitcmp*_b64`, `image_sample_o`, `image_load_pck`, signed atomics, packed inline constants, round-half-even, -0/+0 ordering in min/max, sample-frequency shading and more typed-buffer formats. PlayStation-project sources were removed and re-derived from the AMD RDNA2 ISA.
- **Launch:**
  - A neon loading screen shows until the game's first frame (`NEONBRIDGE_SPLASH=0` turns it off). The game window is kept on one monitor at 16:9, and the game ends when the launcher does (`NEONBRIDGE_JOB=0`).
  - Started without `--game`, the launcher opens a picker for the game folder or exe.
  - It warns when free Windows commit is under 20 GB (`TUXBOX_MIN_COMMIT_GB`), relaunches when the game asks, and names any missing DLL after a loader error.

## Diagnostics

- **`--debug`:** runs under cdb and dumps registers, stacks, the memory region and code around a fault. Title `int 3` asserts are logged and stepped over.
- **Run summary:** the launcher prints one line with fps, the per-frame cost split (record, close, submit, present, waits), the most expensive recording calls, lit-frame samples and failure counters. `--timeout S` ends a run cleanly, and `--summary LOG` summarises an old log.
- **Heartbeat:** the present heartbeat logs fps every 10 s, plus GPU latency per frame and each thread's CPU time. An fps overlay shows in the game window, and `TUXBOX_FPS=0` hides it.
- **Diagnostic switches:**
  - `TUXBOX_TRACE_PRESENT=60` samples whether frames are lit.
  - `TUXBOX_LIST_RING=1` logs the last list calls when a host `Close` fails.
  - `TUXBOX_BARRIER_PROBE=1` closes each barrier alone to name a field the host rejects.
  - Each crash theory has its own `TUXBOX_*` switch, so variants can run from one build.
- **Input logs:** GameInput logs when the title reads the pad and which buttons are down.
- **Crash record:** any unhandled exception, Rust panic or fail-fast logs its code, module and offset, registers and stack, and every thread's stack is logged after 15 s without a frame (`TUXBOX_CRASH_RECORD=0` turns it off).
- **Hot threads:** the heartbeat samples where the busiest threads spend their time and names the functions.
- **Audio probe:** logs the audio endpoints the title opens (spatial audio stream format, buffer and period gaps).
- **Shader bisect:** `TUXBOX_FALLBACK=flat`, `TUXBOX_NULL_SHADERS` and `TUXBOX_NULL_BISECT` swap out individual shaders to find a bad one.
- **Game CLI:** `neonbridge ctl status | log [N] [filter] | stack | press A+Down [ms] | hold | release | screenshot FILE | get | set | unset | stop` controls a running game over a local pipe (`TUXBOX_CTL=0` turns it off; no network).
- **Crash file:** crash records and hang stacks also go to `%LOCALAPPDATA%\6ixpack\logs\crash_<pid>.log`, even when the game was started without the launcher. They name the import or export a crash went through, the instruction bytes and what the faulting address is.
- **Breakpoints:** `TUXBOX_BREAK_RVAS=rva,...` logs registers and 8 stack words each time the title passes those addresses.
- **More logs:** null bindings counted by cause, GPU memory waits that stall (capped per address), waits on unsignalled fences, the wait/write order after a timeout, texture size queries (`TUXBOX_SIZE_WATCH`), and Opus, XDsp and XMA audio counters.

## Open issues

- **Audio:** plays in 3 s bursts. The `chad` fix is the likely cure and needs a run.
- **Frame rate:** about 15-16 fps. Import caching for per-frame buffers is in and needs a run; placed-resource imports still re-open memory.
- **Loading screen:** the game reaches it; a GPU import at title RVA 0x4ecc821 still fails fast later in loading and is being given the CBV-style fallback. Other projects' next known stops are a stream-buffer overflow at about 98 s and a bad free at about 124 s.
- **BCPack textures:** shown as grey blocks until a decoder is written (about 14% of textures).
- **Later screens:** blend patches from derived pipelines are logged but not applied yet.
- **Shader translation:** offline `--build-shaders` still refuses some programs, mainly dynamic `s_load` offsets and mesh shaders.
- **B5G5R5A1 UAVs:** the shader writes float colour into an R16_UINT view, so the 5551 packing is wrong.

## Credits

Neonbridge stands on other people's work. GPL-3.0 and MIT code is used with credit. GPL-2.0-only projects (xcl, rebox) are re-implemented from their behaviour, except xcl's superseded-texture page pool, which is copied (see below). Thank you to:

- **[TuxBox](https://github.com/brcly/TuxBox)** by brcly: the console-DLL providers, D3D12 runtime and `xshader` translator, vendored under `vendor/tuxbox` (Apache-2.0), plus many later upstream fixes ported in, including the title-assert step-over and one-dword raw buffer transfers.
- **[VIctory](https://github.com/Marvisak/VIctory)** by Marvisak: typed null descriptors, async pipeline building, argument clamps and per-frame pipeline budgets, the fine timer for GTA VI's audio mixer, and the list of what the menus need (UI runtime, relaunch, low memory placement, commit).
- **[xcl](https://github.com/ivall/xcl)** by ivall: page-split copies and zero-filled placed uploads, triple-buffered presentation, the resize and memory-wait designs, unknown slots answering instead of failing, the finest timer resolution for choppy audio, XAudio2 creation clean-up and blocking the title's network. Its 64 KiB video-memory page pool for placed textures, with superseded-texture unmapping and remapping (`unmap_superseded`, `remap_if_unmapped`, the 2 s idle and cool-down delays, the mapping fence before submits), is copied from `crates/xcl/src/hle/d3d12x/vram.rs` (commit 7695539, from ad79932/38b05b7; the insert/remove rules of `title_textures.rs`) into `vendor/tuxbox/runtime/d3d12/src/device/placement/pages.rs`, which keeps xcl's credit and its GPL-2.0-only licence header.
- **[Xbox2Windows](https://github.com/R0Y00/Xbox2Windows)** by R0Y00: sampler-heap bounds, shader-translation cross-checks, scalar bit-count instructions, flat integer parameters and the derived-pipeline blend patches.
- **[xbox-emulator](https://github.com/omonkeyz/xbox-emulator)** by omonkeyz (GPL-3.0): fence handling on device removal, network blocking, and span aliasing for placed textures, copied from `crates/compat/d3d12_xs/src/alias.rs` (commit 27122f7) into `vendor/tuxbox/runtime/d3d12/src/device/placement/alias.rs` with its credit header; console core affinity, copied from `crates/compat/compat-common/src/os_host/affinity.rs` into `vendor/tuxbox/runtime/xmem/src/affinity.rs`; upload-buffer recycling and the 48 kHz mastering rate.
- **[OpenScarlett](https://github.com/californiaAlmonds/OpenScarlett)** by californiaAlmonds: rendering fixes and the game-save layouts.
- **[rebox](https://github.com/puffnfresh/rebox)** by puffnfresh: stalled GPU memory-wait diagnostics and the 2 s watchdog, CPU copies into memory the GPU can't reach, dropping `DispatchX`, and network blocking.
- **[palmetto](https://github.com/luizwidmer/palmetto)** by luizwidmer: queue slot 35 as a clock calibration, and per-lane wave tests in `xshader`.
- **[leonida-ce](https://github.com/IAmNotSiob/leonida-ce)** by IAmNotSiob: which extended list options and timestamp writes are safe to ignore or emulate, and the game-save shim layouts.
- **[KestrelX](https://github.com/Batushn/KestrelX)** by Batushn (MIT): compiling `[WaveSize]` out per wave width so kernels run on the host wave.
- **[GTA6-BootTest](https://github.com/OnlyMoisties/GTA6-BootTest)** by OnlyMoisties: notes on what later screens need, such as blend state from the render-state block.
- **[Leonida](https://github.com/ubiquitarianism/leonida)** by ubiquitarianism: shader-translation ports in `xshader`.
- **[Leonida](https://github.com/LeonidaReforge/Leonida)** by LeonidaReforge: the retail boot chain that sized the device table.
- ~~**[leonida-anyp5](https://github.com/m3ll0wSm0ke/leonida-anyp5)**: lowering lane reads and writes to subgroup shuffles in `xshader`.~~
- **Microsoft**: the public GDK headers and documentation (XGameSave, XUser, GameInput, D3D12) that the slot layouts were checked against, and dxc.
