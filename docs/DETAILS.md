# Neonbridge: technical details

The [README](../README.md) has the short version. This page lists what works, every runtime fix, the diagnostic switches and the open issues.

## Status

What the Xbox Series build of GTA VI (`game_scarlett_master.exe`, from your own dump) does today:

- **Boot:** it runs natively with the console DLLs replaced by Rust providers, passes `xgameruntime` initialisation, maps the console `G:` drive to the executable's folder and mounts every `.rpf` archive.
- **Rendering:** it creates a D3D12 device and draws lit frames in a window fitted to one monitor at 16:9: the GTA stars intro, the player-selection prompt, then the menus beyond it. There's no white flash at start: the window stays hidden until its first frame.
- **Speed:** peaks near 40 fps after loading, sinking to about 20 as video memory runs over budget.
- **Input:** the title reads the virtual pad, and an automatic A press gets it past the player-selection prompt.
- **Saves:** XGameSave containers are stored under `%LOCALAPPDATA%\TuxBox\PersistentLocalStorage\<title id>\GameSave\`, and the game loads its "Player" save on the next launch.
- **Offline and contained:** the game's network calls fail as "no network", its `D:` drive is a local folder, and it runs in a job that ends with the launcher.
- **Audio:** sound plays, and Bink movies keep to their clock.
- **Menus:** dialogs, text entry, speech and relaunch requests are answered, so the menus no longer wait on missing console UI. The game gets through the menus and loading screens into the first mission, where the picture is still black.
- **Longest run:** about 4-5 minutes (frame ~3100), ending in a 15 s freeze. One cause, a deadlock in the placement code, is fixed; the freeze still happens, so a second wait is being traced.

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
  - `XErrorReport` calls the callback set with `XErrorSetCallback` (`TUXBOX_XERROR_CALLBACK=0` never calls it).
- **Audio:**
  - The title asks for a 1 ms (or finer) timer (`TUXBOX_FINE_TIMER=0`).
  - `chad` (XDsp) completes every command, with silence for transforms it can't compute (`TUXBOX_CHAD_STRICT=1`).
  - AcpHal answers connect, message queue and XMA submit, with an XMA decoder (LGPL `xma` crate, notice in `vendor/tuxbox/runtime/acphal/NOTICE`; `TUXBOX_XMA=0`).
  - `ApuFree` gives blocks back to the APU heap through a coalescing free list (`TUXBOX_APU_FREE=0` makes it a no-op).
  - The title's audio-lock leak at game+0x5267720 is patched when its bytes match (`TUXBOX_AUDIO_LOCK_PATCH=0` off).
  - Bink movies are held at their clock, `TUXBOX_BINK_LEAD` (1) frames ahead, decoding up to `TUXBOX_BINK_CATCHUP` (4) a call, also on `BinkWait` (`TUXBOX_BINK_WAIT_CATCHUP=0` only on `BinkNextFrame`).
- **Window and keys:** Alt and F10 no longer open the modal window menu that made the game abort (`TUXBOX_KEY_MENU=1`).
- **Shader translator:** many more instructions and fixes, among them `v_permlane16`, `v_mad_i64_i32`, `s_memtime`, `s_bitcmp*_b64`, `image_sample_o`, `image_load_pck`, signed atomics, packed inline constants, round-half-even, -0/+0 ordering in min/max, sample-frequency shading and more typed-buffer formats. PlayStation-project sources were removed and re-derived from the AMD RDNA2 ISA.
- **Depth-stencil barriers:**
  - Depth-stencil textures leave layout DIRECT_QUEUE_GENERIC_READ for DEPTH_STENCIL_READ (`TUXBOX_DS_GENERIC_READ=1` keeps it).
  - In DEPTH_STENCIL_READ layout a depth-stencil side drops SHADER_RESOURCE access (`TUXBOX_DS_READ_SR=1` keeps it), and a side whose access is only depth-stencil gets DEPTH_STENCIL sync (`TUXBOX_DS_SYNC=0` off).
  - Barriers over both planes of a two-plane depth format are recorded once per plane (`TUXBOX_DS_PLANE_SPLIT=0` keeps one).
  - A texture barrier the host rejects is retried one field at a time, and the variant it accepts is applied to later barriers of the same kind (`TUXBOX_BARRIER_LEARN=0` logs only). One that no variant fixes is left out so `Close` doesn't fail (`TUXBOX_BARRIER_SKIP=0` records it).
- **Lists and batches:**
  - A list that fails to record, or whose BLAS preparation fails, is left out of its batch instead of dropping the batch (`TUXBOX_STRICT_LISTS=1`, `TUXBOX_STRICT_SCENE=1` drop it). Its marker and 64-bit value writes are then made by the CPU after the batch, so the title's GPU-progress wait doesn't hang (`TUXBOX_ORPHAN_WRITES=0` off).
  - A batch whose written placed texture overlaps another of its textures runs, sharing bytes as on the console (`TUXBOX_OVERLAP_STRICT=1` drops it).
  - A page-mapping call xmem refuses whole is applied batch by batch (`TUXBOX_PAGE_MAPPING_STRICT=1` refuses it whole).
  - Slot-35 submissions are taken on every queue type (`TUXBOX_SLOT35_DIRECT_ONLY=1` drops others), and an array holding no list of ours is dropped instead of having clock ticks written over it (`TUXBOX_QSLOT35=pair` writes them).
  - A null root CBV/SRV/UAV points at a 64 KiB zeroed buffer instead of GPU VA 0 (`TUXBOX_NULL_ROOT_SINK=0` binds 0).
  - A memory copy within one buffer goes through a scratch buffer, up to `TUXBOX_COPY_SCRATCH_MB` (64; 0 off).
  - The fence completion cell only rises (`TUXBOX_FENCE_CELL_MAX=0` stores as read).
  - Each guest queue uploads placed textures on its own copy queue (`TUXBOX_UPLOAD_PER_QUEUE=0` shares one), and a queue no longer waits on another queue's write-back or reads (`TUXBOX_PLACED_CROSS_WAITS=1` restores them).
- **GPU memory waits:**
  - A wait is released once the list that writes its value has completed on the GPU (`TUXBOX_WAIT_WRITER_RELEASE=0` waits for the 2 s watchdog).
  - A wait whose word can't be mapped is dropped instead of refusing its list (`TUXBOX_WAIT_MAP_STRICT=1` refuses it).
- **Present throttle:** each present waits until every guest queue is within `TUXBOX_PRESENT_LAG` (2) presents of it, at most `TUXBOX_PRESENT_WAIT_MS` (5000) per queue (`TUXBOX_PRESENT_LAG=0` off). TYPELESS RGBA8, BGRA8 and RGB10A2 surfaces present through their UNORM back buffer (`TUXBOX_PRESENT_TYPELESS=0` refuses them).
- **Video memory and page pools:**
  - Released reserved resources are reused by description (`TUXBOX_PAGES_RECYCLE`, 4096; 0 keeps none), the pages fence is signalled once before the next title wait (`TUXBOX_PAGES_SIGNAL_EACH=1` signals each mapping), and an import a live heap covers exactly reuses it (`TUXBOX_IMPORT_REUSE=0` off).
  - Page pools are no longer evicted or put in system memory, since a shader can still sample an evicted pool through the descriptor heap (`TUXBOX_PAGES_EVICT=1` evicts idle pools again past `TUXBOX_SPAN_EVICT_PCT` of budget, and `TUXBOX_PAGES_SYSTEM_PCT=80` puts new sampled pools in system memory again). A pool idle for `TUXBOX_SPAN_EVICT_S` (10 s) over budget is demoted to the lowest residency priority instead (`TUXBOX_PAGES_DEMOTE=0` off).
  - Pools with no tile in use are evicted while over budget (`TUXBOX_PAGES_TRIM_EMPTY=0` off), and tiles come from the fullest pools first so sparse pools drain (`TUXBOX_PAGES_PACK=0` keeps the old order).
  - Sampled textures placed over `TUXBOX_PAGES_TRIM_PCT` (95) percent of budget leave mip 0 unmapped, and over `TUXBOX_PAGES_TRIM2_PCT` (110) mips 0 and 1 (0 maps every mip).
  - Over budget, placed textures idle for `TUXBOX_PAGES_IDLE_S` are unmapped, largest first and none under `TUXBOX_PAGES_IDLE_MIN_KB` (1024), then mapped again with a full upload at their next use (`TUXBOX_PAGES_IDLE_S=0` never unmaps).
  - A texture goes on pages only if its format has tiled support (`TUXBOX_PAGES_TILED_CHECK=0` skips the check).
  - Sampled-only render heaps get LOW residency priority, heaps with render or depth targets HIGH (`TUXBOX_RENDER_HEAP_PRIORITY=0` off).
  - Slot 108 answers a description the host can't size with a conservative size instead of u64::MAX (`TUXBOX_SLOT108_FALLBACK=0` answers invalid).
- **Placed-texture refresh and write-back:**
  - Each level keeps a 64-bit hash of its title bytes instead of a copy, and a texture whose pages go to an identical texture at the same address keeps its snapshots (`TUXBOX_CLAIM_SAME_KEEP=0` re-reads on every claim).
  - Levels over `TUXBOX_SNAPSHOT_BIG_MB` (16) are read back only after a CPU write, at most every `TUXBOX_SNAPSHOT_BIG_MS` (500). Levels over `TUXBOX_SNAPSHOT_MAX_MB` (256) are never read back (0 lifts it) or written back (`TUXBOX_WRITEBACK_MAX_MB=0` writes them).
  - A refresh reads at most `TUXBOX_REFRESH_READ_MB` (32) and `TUXBOX_REFRESH_READ_MS` (4), and the next starts where it stopped (`TUXBOX_REFRESH_ROTATE=0` always starts at the first texture).
  - Live write-back readback buffers stay within `TUXBOX_WRITEBACK_INFLIGHT_MB` (512; 0 lifts it).
  - A texture inherits another's GPU readers once per fence instead of doubling its list (`TUXBOX_READERS_MERGE=0` appends).
  - Out of memory in residency, refresh or write-back retries after freeing dead shadows (`TUXBOX_PLACED_OOM_RETRY=0` off), then skips with stale bytes (`TUXBOX_PLACED_OOM_STRICT=1` drops the batch).
  - Watched ranges under `TUXBOX_WATCH_GAP_MB` (64) apart are one write-watch scan (0 scans each alone).
  - A placed texture's Drop no longer deadlocks on the page index when an overlapping texture's last reference goes with it (one cause of the 15 s freeze at the end of a run).
- **Freezes on imports and shadows:**
  - One thread opens a given range as a heap at a time; the rest take its heap (`TUXBOX_IMPORT_SINGLE=0` off).
  - An import over pages from more than one OS allocation goes straight to the shadow copy (`TUXBOX_IMPORT_PRECHECK=0` asks the driver), refused ranges are remembered by page for about 1 s (`TUXBOX_REFUSED_PAGES=0` off), and the reservation walk is bounded (`TUXBOX_RESERVATION_WALK_ALL=1` walks to the end).
  - Shadows are hashed (`TUXBOX_SHADOW_HASH_LOCKED=1`) and released (`TUXBOX_SHADOW_DROP_LOCKED=1`) outside the shadow lock, pruned only after the map grows (`TUXBOX_SHADOW_PRUNE_EVERY=1` scans every call), and dead ones are freed before a remap of their range (`TUXBOX_SHADOW_RELEASE_ON_REMAP=0` keeps them).
  - A descriptor publish walks only the tables that can hold its slots (`TUXBOX_PUBLISH_SCAN_ALL=1` walks them all).
- **xmem page mapping:**
  - Unmapping a page inside a multi-page or unrecorded view splits or rebuilds that view (`TUXBOX_XMEM_UNMAP_FALLBACK=0` off).
  - An unmap failing with error 87 is deferred and retried later instead of refusing its batch (`TUXBOX_XMEM_DEFER_UNMAP=0` refuses it); the short sleep-retry is off (`TUXBOX_XMEM_UNMAP_RETRY=1` on).
  - The one-trap CLZERO fill loop leaves the full EFlags of its last compare (`TUXBOX_CLZERO_FLAGS=0` only clears CF).
- **Pipelines:**
  - A pipeline an earlier run bound goes to the workers at once instead of compiling inside the title's Create call (`TUXBOX_EAGER_ON_TITLE=1` compiles it there).
  - Bound pipelines go ahead of the workers' queue (`TUXBOX_PIPELINE_PRIORITY=0` off), and a frame waits at most 50 ms for pipelines still being made (`TUXBOX_FRAME_WAIT_MS=500` as before).
  - Compute pipelines up to `TUXBOX_SMALL_COMPUTE_BYTES` (16384; 0 off) are made on the title's thread at first bind, drawing on a shared first-use budget of 0.25 s a second up to 1 s (`TUXBOX_FIRST_USE_BUDGET=0` off). A worker building a pipeline the title waits on runs at normal priority (`TUXBOX_PIPELINE_HURRY=0` off).
  - Driver compiles wait for the memory they're expected to take (4096 bytes per byte of DXIL, 2-32 GB), at most `TUXBOX_COMPILE_BUDGET_WAIT_MS` (30000) (`TUXBOX_COMPILE_BUDGET=0` off).
- **Views and Map:** a depth image gets a default SRV of its depth plane (`TUXBOX_DEPTH_DEFAULT_SRV=0` off), and Map of a resource with no CPU pages answers E_INVALIDARG without the PC call (`TUXBOX_MAP_HEAP_CHECK=0` off).
- **DirectStorage (`xcrdapi`):** a request fails when its destination isn't committed and writable (`TUXBOX_XCRD_CHECK_DEST=0` off), and a read refused at enqueue keeps its request, so its error record names it and a tag cancel reaches it (`TUXBOX_XCRD_REFUSED_RECORD=0` off).
- **Shader translator, more:**
  - `v_div_scale_f32`, `v_div_fmas_f32` and `v_div_fixup_f32` (`TB_DIV_OPS=0` refuses them).
  - `image_atomic` sub, inc, dec, fmin, fmax, cmpswap and fcmpswap (`TB_IMAGE_ATOMIC_EXT=0` refuses them).
  - Cube UAVs addressed as 2D arrays with face = z (`TB_CUBE_UAV=0` refuses them).
  - `v_pk_add_i16`, `v_pk_mad_i16/u16`, `v_pk_max/min_u16` and the packed clamp bit (`TB_PK_INT16_MORE=0` refuses them).
  - Indexed buffer accesses past their element are out of range as on RDNA: loads read 0, stores drop (`TB_STRUCT_OFFSET_OOB=0` addresses flat).
  - `v_writelane_b32` keeps its other lanes live (`TB_WRITELANE_CARRY=0` counts a full write), and every address VGPR of a non-NSA image instruction counts as read (`TB_MIMG_ADDR_LIVE=0` counts vaddr only).
  - `s_ff1_i32_b64` on any operand and MTBUF format 69 as four 16-bit channels (`TB_FF1_FMT69=0` off); wide image atomics in every pixel shader (`TB_WIDE_ATOMIC_ANY_PS=0` HWRasterizePS only).
  - Inline sampler words through `s_mov_b64` pair copies (`TUXBOX_SAMPLER_PAIR_MOVES=0` refuses them) and packed RGBA8 `image_store_pck`/`image_store_mip_pck` (`TUXBOX_PACKED_STORES=0` refuses them).
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
  - `TUXBOX_BARRIER_PROBE=1` closes each barrier alone to name a field the host rejects. Each try gets a fresh allocator and list, and a barrier no variant fixes has its resource probed too.
  - On a failed host `Close`, the list's last 8 enhanced barriers are logged with any D3D12 rule they break, and the probe runs on the next groups (`TUXBOX_BARRIER_RING=0` off).
  - Each crash theory has its own `TUXBOX_*` switch, so variants can run from one build.
- **Input logs:** GameInput logs when the title reads the pad and which buttons are down.
- **Crash record:** any unhandled exception, Rust panic or fail-fast logs its code, module and offset, registers and stack, and every thread's stack is logged after 15 s without a frame (`TUXBOX_CRASH_RECORD=0` turns it off).
- **Hot threads:** the heartbeat samples where the busiest threads spend their time and names the functions.
- **Audio probe:** logs the audio endpoints the title opens (spatial audio stream format, buffer and period gaps).
- **Shader bisect:** `TUXBOX_FALLBACK=flat`, `TUXBOX_NULL_SHADERS` and `TUXBOX_NULL_BISECT` swap out individual shaders to find a bad one.
- **Game CLI:** `neonbridge ctl status | log [N] [filter] | stack | press A+Down [ms] | hold | release | screenshot FILE | get | set | unset | stop` controls a running game over a local pipe (`TUXBOX_CTL=0` turns it off; no network).
- **Crash file:** crash records and hang stacks also go to `%LOCALAPPDATA%\6ixpack\logs\crash_<pid>.log`, even when the game was started without the launcher. They name the import or export a crash went through, the instruction bytes and what the faulting address is.
- **Breakpoints:** `TUXBOX_BREAK_RVAS=rva,...` logs registers and 8 stack words each time the title passes those addresses.
- **Debug layer:** `TUXBOX_D3D12_SDK=<dir>` makes a debug device through that Agility SDK's device factory, with InfoQueue capture; if it fails, the normal device is made. `TUXBOX_D3D12_SDK_GLOBAL=1` also calls `SetSDKVersion` first (process-wide, needs Developer Mode), and a device refused under it is returned, not retried on WARP. `TUXBOX_D3D12_SDK_VERSION` overrides the version read from the SDK's D3D12Core.dll.
- **DRED:** `TUXBOX_DRED=1` names every command list for its source (title, rebuilt, or the runtime's own upload, readback, copy, wait or present lists), and logs the completed lists just before the first incomplete one.
- **Frame work:** after each 10 s heartbeat, a line counts presents, draws and dispatches called and recorded, and why the rest were dropped (`TUXBOX_FRAME_WORK=0` off).
- **Memory line:** live render heaps (count and MiB), commit split into title and host, merged write-watch scans, recycled resources and pool evictions. Every power of two of render heaps from 64, it counts older heaps still alive under a newer one, and a packing log reports pools with no tile in use.
- **Render-heap eviction (opt-in):** `TUXBOX_RENDER_HEAP_EVICT=1` records each render heap's use and evicts ones unused for `TUXBOX_SPAN_EVICT_S` while over budget. It is off by default because the tracking cut 35-40 fps to 15.
- **Stalls:** every queue Wait is recorded, so a stalled GPU memory wait names what its writer's queue waits on (`TUXBOX_QUEUE_WAITS=0` off), and says when its word's imported range was remapped. The present-throttle line gives the tracked queues and how far behind the furthest is; compile-budget waits and import waits are logged.
- **Failure logs:** a refused `CreateDescriptorHeap` logs its description (`TUXBOX_HEAP_FAIL_LOG=0` off); scene (BLAS) preparation failures say which check refused; a dropped batch names its step; xmem's page-mapping failures name the last failed OS step and the GPU imports holding the page.
- **More logs:** null bindings counted by cause, GPU memory waits that stall (capped per address), waits on unsignalled fences, the wait/write order after a timeout, texture size queries (`TUXBOX_SIZE_WATCH`), and Opus, XDsp and XMA audio counters.

## Open issues

- **Video memory:** the main limit. The game uses about 8 GB against a 7.2 GB device-local budget, and the frame rate sinks from about 40 to 20 once it overflows.
- **Black picture:** the first mission plays sound but draws black.
- **End-of-run freeze:** after about 3000 frames no frame is presented for 15 s; the title's submit thread waits on a command list's recording lock. The placement deadlock fix didn't end it.
- **BCPack textures:** shown as grey blocks until a decoder is written (about 14% of textures).
- **Later screens:** blend patches from derived pipelines are logged but not applied yet.
- **Shader translation:** offline `--build-shaders` still refuses some programs, mainly dynamic `s_load` offsets and mesh shaders.
- **B5G5R5A1 UAVs:** the shader writes float colour into an R16_UINT view, so the 5551 packing is wrong.

## Credits

Neonbridge stands on other people's work. GPL-3.0 and MIT code is used with credit. GPL-2.0-only projects (xcl, rebox) are re-implemented from their behaviour, except xcl's page-pool code, which is copied (see below). Thank you to:

- **[TuxBox](https://github.com/brcly/TuxBox)** by brcly: the console-DLL providers, D3D12 runtime and `xshader` translator, vendored under `vendor/tuxbox` (Apache-2.0), plus many later upstream fixes ported in, including the title-assert step-over, one-dword raw buffer transfers, packed 16-bit integer ops and the clamp bit (fae402f, 34ba13a), `s_ff1_i32_b64` and MTBUF format 69 (d575b34), wide image atomics in every pixel shader (2a7fd48) and sampler words through `s_mov_b64` pair copies (4cd8d1c).
- **[VIctory](https://github.com/Marvisak/VIctory)** by Marvisak: typed null descriptors, async pipeline building, argument clamps and per-frame pipeline budgets, the fine timer for GTA VI's audio mixer, and the list of what the menus need (UI runtime, relaunch, low memory placement, commit).
- **[xcl](https://github.com/ivall/xcl)** by ivall: page-split copies and zero-filled placed uploads, triple-buffered presentation, the resize and memory-wait designs, unknown slots answering instead of failing, the finest timer resolution for choppy audio, XAudio2 creation clean-up and blocking the title's network. Its 64 KiB video-memory page pool for placed textures, with superseded-texture unmapping and remapping (`unmap_superseded`, `remap_if_unmapped`, the 2 s idle and cool-down delays, the mapping fence before submits), is copied from `crates/xcl/src/hle/d3d12x/vram.rs` (commit 7695539, from ad79932/38b05b7; the insert/remove rules of `title_textures.rs`) into `vendor/tuxbox/runtime/d3d12/src/device/placement/pages.rs`, which keeps xcl's credit and its GPL-2.0-only licence header. The tiled-support check for textures on pages is copied there too (24d8b92 with 38ea451). Re-derived ideas: the default depth SRV (615cb20), the Map heap check (21de1a8), marking reserved resources (cf244b0, 7d834d4), keeping refused DirectStorage reads (66660bd, 2e17d3d), same-buffer copies through scratch (21bc548, 392dc0c), raising a waited-on pipeline worker (25c6f7e, 9a272d4) and merging write-watch scans (712bbbe).
- **[Xbox2Windows](https://github.com/R0Y00/Xbox2Windows)** by R0Y00: sampler-heap bounds, shader-translation cross-checks, scalar bit-count instructions, flat integer parameters and the derived-pipeline blend patches; typeless surfaces presenting (99624a8d); and, ported under MIT, the coalescing `ApuFree` (64b6d2d3, f9033a9b) and packed RGBA8 image stores (b898d318).
- **[xbox-emulator](https://github.com/omonkeyz/xbox-emulator)** by omonkeyz (GPL-3.0): fence handling on device removal, network blocking, and span aliasing for placed textures, copied from `crates/compat/d3d12_xs/src/alias.rs` (commit 27122f7) into `vendor/tuxbox/runtime/d3d12/src/device/placement/alias.rs` with its credit header; console core affinity, copied from `crates/compat/compat-common/src/os_host/affinity.rs` into `vendor/tuxbox/runtime/xmem/src/affinity.rs`; upload-buffer recycling and the 48 kHz mastering rate.
- **[OpenScarlett](https://github.com/californiaAlmonds/OpenScarlett)** by californiaAlmonds: rendering fixes and the game-save layouts.
- **[rebox](https://github.com/puffnfresh/rebox)** by puffnfresh: stalled GPU memory-wait diagnostics and the 2 s watchdog, CPU copies into memory the GPU can't reach, dropping `DispatchX`, and network blocking; re-derived ideas for failing storage requests into unwritable memory (f61bbc8), remembering refused import ranges (19a69d0) and bounding the reservation walk (90b6578, a85c353).
- **[palmetto](https://github.com/luizwidmer/palmetto)** by luizwidmer: queue slot 35 as a clock calibration, and per-lane wave tests in `xshader`; under GPL-3.0, the conservative slot-108 size (cc7e044b), the rising fence cell (77d2f6bb) and slot 35 as the console's submit (22555b24).
- **[leonida-ce](https://github.com/IAmNotSiob/leonida-ce)** by IAmNotSiob: which extended list options and timestamp writes are safe to ignore or emulate, and the game-save shim layouts; under GPL-3.0, the audio-lock patch (62ad4fdf), the descriptor-heap failure log (1403e90e), the `v_div_*` semantics, the `image_atomic` ops of its GPU fixture (367d91ef) and cube UAV stores (0d27fdcf).
- **[KestrelX](https://github.com/Batushn/KestrelX)** by Batushn (MIT): compiling `[WaveSize]` out per wave width so kernels run on the host wave, and `XErrorReport` with its callback (ee0f749).
- **[five-star-recomp](https://github.com/ThatRecompDev/five-star-recomp)** by ThatRecompDev (GPL-3.0): the present throttle (4cc3094), making small compute pipelines at first bind (8fa5672), the compile budget (afdd699, CompileBudget), the first-use budget (ab94e3c, FirstUseBudget.cs) and RDNA range checks on indexed buffer accesses (3d287ab).
- **[GTA6-BootTest](https://github.com/OnlyMoisties/GTA6-BootTest)** by OnlyMoisties: notes on what later screens need, such as blend state from the render-state block.
- **[Leonida](https://github.com/ubiquitarianism/leonida)** by ubiquitarianism: shader-translation ports in `xshader`, among them `v_writelane_b32` lane liveness (f05bc538) and image address registers (7874db15), and the CLZERO loop's compare flags (3b8c1555, apply_cmp_flags).
- **[Leonida](https://github.com/LeonidaReforge/Leonida)** by LeonidaReforge: the retail boot chain that sized the device table.
- ~~**[leonida-anyp5](https://github.com/m3ll0wSm0ke/leonida-anyp5)**: lowering lane reads and writes to subgroup shuffles in `xshader`.~~
- **Microsoft**: the public GDK headers and documentation (XGameSave, XUser, GameInput, D3D12) that the slot layouts were checked against, and dxc.
