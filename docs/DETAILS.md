# Neonbridge: technical details

The [README](../README.md) has the overview. This page covers the components, building and running, every environment switch, and the credits.

## Components

- **`neonbridge` (launcher, `src/`):** finds the dump (or opens a picker), builds the providers and `xshader` from `vendor/tuxbox`, stages them, sets safe game settings, runs the shader worker (prewarm, title requests, dxc in-process or `dxc.exe`, `-O0` to `-O1` upgrade queue), shows the loading screen, keeps displays awake, runs the game in a job object and serves `neonbridge ctl` over a local pipe.
- **`crates/feed`:** the opt-in game feed (fMP4 via ffmpeg, MJPEG fallback) on 127.0.0.1:8788, optionally shared over Tailscale (`tools/tsproxy.exe`) or cloudflared.
- **`vendor/tuxbox/runtime/*` (TuxBox providers):** Rust replacements for the console DLLs.
  - `d3d12`: D3D12.x on desktop D3D12: list and queue slots, barriers, placed textures and page pools, root and address views, imports and shadow copies, GPU memory waits, pipelines and the shared pipeline library, presentation, diagnostics and frame capture.
  - `xmem`: title memory, lazy commit on first touch, page mappings, commit bits, title memory cap, core affinity.
  - `xgameruntime`: XGameSave, XGameUi, XAccessibility, XAsync, XError, relaunch.
  - `xcrdapi`: DirectStorage reads and decompression.
  - `acphal`, `xapu`: audio (XMA decoder, XAPU, `chad` XDsp).
  - `gameinput`: pads and the keyboard-as-pad; `xg`, `xg-layout`: console texture layouts; `bvh`: ray-tracing builds; `dxbc`, `dxc`: shader containers and compiler loading.
- **`vendor/tuxbox/tools/shader-compiler` (`xshader`):** translates RDNA2 shader code to HLSL for dxc. `xshader --refusals DIR [--json]` lists the first refusal of every program in a request folder.

## Build and run

Needs Rust stable (`x86_64-pc-windows-msvc`), Visual Studio Build Tools (x64 C++) and the Windows SDK (`dxc.exe`; `cdb` for `--debug`). The game folder needs x64 `dxcompiler.dll` and `dxil.dll`.

```
cargo build --release
neonbridge --game <dump dir> [--debug] [--log FILE] [--timeout S] [--rev COMMIT]
neonbridge --game <dir> --prewarm       # translate the seen shader set and exit
neonbridge --game <dir> --pipelines     # build pipelines.journal into the library (never while the game runs)
neonbridge --summary LOG                # summarise an old log
neonbridge --feed-only                  # keep the feed page up with no game
neonbridge replay-shaders <capture dir> [--xshader EXE] [--against DIR]
neonbridge ctl status | log [N] [filter] | stack | press A+Down [ms] | hold | release | screenshot FILE | get | set | unset | capture [K] | stop
```

- The launcher builds `xshader` from the tree at launch and keeps the last good build as `xshader-good.exe`; with no good copy, no dxc or no worker, the launch stops.
- `--rev` builds providers and `xshader` from that commit's `vendor/tuxbox`, mirrored into `6ixpack/rev-build/vendor/tuxbox`.
- Run `neonbridge --prewarm` after every `xshader` change so the next run starts warm.
- Shader cache: `tuxbox_shader_cache/` (with `seen`, `last/` and `dxil/<dxc fingerprint>/`); the pipeline library, `pipelines.journal` and `pipeline-names.txt` live in `runtime/`.
- Saves: `%LOCALAPPDATA%\TuxBox\PersistentLocalStorage\<title id>\GameSave\`. Crash logs: `%LOCALAPPDATA%\6ixpack\logs\crash_<pid>.log`. Feed log: `%LOCALAPPDATA%\6ixpack\feed\feed.log`. Captures: `%TEMP%\tuxbox_capture\<pid>-frame<N>`.
- `--debug` runs under cdb and dumps registers, stacks and code at a fault. The run summary gives fps, per-frame cost and failure counters; a heartbeat logs every 10 s.

## Environment switches

Format: name (default): effect of changing it. "on" means the fix runs unless set to 0.

### Launcher (`NEONBRIDGE_*`)

- `NEONBRIDGE_REV` (unset): same as `--rev`.
- `NEONBRIDGE_REV_INCREMENTAL` (1): 0 builds in the snapshot itself.
- `NEONBRIDGE_SPLASH` (1): 0 hides the loading screen.
- `NEONBRIDGE_JOB` (1): 0 lets the game outlive the launcher.
- `NEONBRIDGE_SAFE_SETTINGS` (1): 0 keeps the user's graphics settings.
- `NEONBRIDGE_KEEP_AWAKE` (1): 0 drops the display-required power request.
- `NEONBRIDGE_WAKE_DISPLAY` (1): 0 skips waking sleeping displays.
- `NEONBRIDGE_DXC_CHECK` (1): 0 skips the missing dxcompiler/dxil warning.
- `NEONBRIDGE_SHADER_THREADS` (prewarm: cores-2, game: 8): shader worker threads.
- `NEONBRIDGE_PREWARM_SEEN` (1): 0 skips the seen-set prewarm.
- `NEONBRIDGE_PREWARM_MAX_S` (600; `--prewarm`/`--pipelines`: 0): prewarm time cap, 0 none.
- `NEONBRIDGE_PREWARM_DXC_O0` (1 while the `-O1` upgrade drainer is on): the prewarm compiles at `-O0` and the drainer upgrades to `-O1`; 0 keeps `-O1` first.
- `NEONBRIDGE_URGENT_SHADERS` (1): 0 serves shaders in directory order.
- `NEONBRIDGE_DXC_URGENT_O0` (1): 0 compiles urgent shaders at `-O1`.
- `NEONBRIDGE_BLOCKING_PREWARM` (0): 1 prewarms the seen set before the game; by default it runs in a background idle-priority thread that stands aside while the title waits on a shader.
- `NEONBRIDGE_DXC_UPGRADE_IN_GAME` (0): 1 runs the `-O1` upgrade queue while the title runs; `NEONBRIDGE_DXC_UPGRADE_DRAIN` (1): 0 skips the detached `--drain-upgrades` that empties it between runs.
- `NEONBRIDGE_PREWARM_UPGRADE_O0` (0): 1 lets the explicit prewarm recompile cached `-O0` DXIL at `-O1`.
- `NEONBRIDGE_DRAIN_COPY` (1): the `-O1` upgrade drainer runs from its own copy (`neonbridge-drain.exe`) so a build's taskkill no longer ends it; 0 runs the launcher's exe. `NEONBRIDGE_DRAIN_YIELD_GAME` (1): the drainer steps aside while the game runs and logs to `drain.log`; 0 off.
- `NEONBRIDGE_XSHADER_SKIP` (1): xshader's cargo run is skipped when its sources hash as at the last good build; 0 always runs cargo. `NEONBRIDGE_XSHADER_PARALLEL` (1): xshader builds beside the runtime build; 0 builds after it.
- `NEONBRIDGE_CACHE_REUSE_PUBLISHED` (1): a cache publish whose rename loses to another worker's identical file uses the published one (VIctory-zhai 37e90b3, after fxceless 43f465b); 0 fails the compile.
- `NEONBRIDGE_SEEN_LINKS` (1): 0 writes empty `seen/` markers instead of hard links to `requests/`.
- `NEONBRIDGE_WATCH_HANG` / `--watch-hang SPEC` (off): samples title globals (game+RVA, 1-8 bytes) every 100 ms from outside the process.
- `NEONBRIDGE_DXC_O1_SECS` (10): `-O1` time before the `-O0` retry.
- `NEONBRIDGE_DXC_INPROC` (1): 0 spawns `dxc.exe` for every compile.
- `NEONBRIDGE_DXC_UPGRADE` (1): 0 keeps `-O0` results.
- `NEONBRIDGE_DXC_UPGRADE_SECS` (300): upgrade compile budget.
- `NEONBRIDGE_TIMELINE` (1): `<log>.timeline.txt` beside each run log, one line per milestone (launch, runtime loaded, first present, BinkOpen, watch-hang, fps every 30 s, exit, and game `TIMELINE:` lines); 0 writes none.
- `NEONBRIDGE_REQUEST_RECHECK` (1): the shader worker skips a request whose file vanished after the listing; 0 treats it as failed.
- `NEONBRIDGE_AUTO_PRESS_ANY_FOCUS` (1): auto and ctl presses reach the title's primary pad when its window is not foreground (real input stays foreground-gated); 0 presses only while foreground. `TUXBOX_AUTO_PRESSES` (1): auto A presses, one every 3 s.
- `NEONBRIDGE_DXIL_REUSE` (1): 0 compiles every program.
- `NEONBRIDGE_DXIL_REUSE_NORMALISED` (1): 0 keys DXIL by exact HLSL text.
- `NEONBRIDGE_XSHADER_SOURCE_KEY` (1): 0 keys the cache on xshader.exe bytes.
- `NEONBRIDGE_WORKER_CEILING_GB` (8): shader worker memory ceiling, 0 off.
- `NEONBRIDGE_PIPELINE_THREADS` (half the cores): `--pipelines` threads.
- `NEONBRIDGE_REPLAY_DXC` (1): 0 skips dxc in `replay-shaders`.
- `NEONBRIDGE_REPLAY_JOBS` (cores): `replay-shaders` workers.
- `NEONBRIDGE_FEED` (0): 1 serves the game feed.
- `NEONBRIDGE_FEED_PORT` (8788), `NEONBRIDGE_FEED_HOST`, `NEONBRIDGE_FEED_FPS`, `NEONBRIDGE_FEED_WIDTH`, `NEONBRIDGE_FEED_KBPS`: feed port, host and encoding.
- `NEONBRIDGE_FEED_LINK` (tailscale): `tailscale`, `cloudflared` or `none`.
- `NEONBRIDGE_FEED_STALE_S` (5): seconds without a frame before the encoder stops, 0 never.
- `NEONBRIDGE_FFMPEG`, `NEONBRIDGE_TSPROXY`, `NEONBRIDGE_CLOUDFLARED` (on PATH / `tools/`): tool paths.
- `NEONBRIDGE_PREWARM_PIPELINES` (1): --prewarm also builds the journal's graphics and compute pipelines when --game names a folder with the staged d3d12_xs.dll; 0 runs dxc only. Its shader session is run-prewarm-<pid>.
- `NEONBRIDGE_PREWARM_JOB_S` (300; 0 off): a prewarm job past it has its dxc/xshader child ended and its key kept as a dxc failure; `NEONBRIDGE_DXC_RETRY_FAILED` (0): 1 retries such keys.
- `NEONBRIDGE_PREWARM_CATCH_PANIC` (1): a panic in one prewarm compile fails that program instead of hanging the progress loop.
- `NEONBRIDGE_DXC_TIMES_LOG` (1): shader-worker.log gets one line per dxc compile (key, ms, HLSL bytes); totals and ten slowest are always written.
- `NEONBRIDGE_TOOL_KILL_LOG` (1): run_until logs each tool it kills at its timeout with the kill's result.

### Launch, platform and input

- `TUXBOX_LOG` (launcher sets `stderr`): log destination.
- `TUXBOX_MIN_COMMIT_GB` (20): free-commit warning threshold.
- `TUXBOX_MIN_COMMIT_MB` (2048): the process ends once free commit drops below this, 0 off.
- `TUXBOX_TITLE_MEMORY_MB` (13824): title memory cap, 0 uncapped.
- `TUXBOX_CONSOLE` (Series X): `s` Series S, `pc` a PC.
- `TUXBOX_FINE_TIMER` (1): 0 keeps the 15.6 ms tick.
- `TUXBOX_NETWORK` (0): 1 allows network.
- `TUXBOX_FULLSCREEN` (0): 1 starts borderless fullscreen.
- `TUXBOX_KEYBOARD_GAMEPAD` (1): 0 leaves only real pads.
- `TUXBOX_AUTO_START` (1): 0 stops the automatic A press.
- `TUXBOX_KEY_MENU` (0): 1 lets Alt/F10 open the window menu.
- `TUXBOX_SUBCLASS_WINDOW` (1): 0 relies on the DefWindowProc import alone.
- `TUXBOX_HIDDEN_TITLE_WINDOW` (1): 0 presents to our window.
- `TUXBOX_ADOPT_RULE` (1): 0 adopts the largest visible window.
- `TUXBOX_CTL` (1): 0 disables the `ctl` pipe.
- `TUXBOX_FEED` (set by the launcher): d3d12 frame tap for the feed.
- `TUXBOX_SSE4A_ZERO_HIGH` (1): 0 leaves extrq/insertq high quadword.
- `TUXBOX_CLZERO_FLAGS` (1): 0 clears CF only.
- `TUXBOX_UNMAPPED` (answer 0): `failfast` fails on unmapped slots.
- `TUXBOX_NO_WATCHDOG` (1): the title's stall watchdog is patched out; 0 keeps it.
- `TUXBOX_SLEEP_HOLD` (100): shortest Sleep held during pipeline waits, ms; 0 off.
- `TUXBOX_RELAUNCH` (1): 0 ignores `XLaunchNewGame`.
- `TUXBOX_PAD_ANY_FOCUS` (1): pads stay connected and readable while unfocused; `TUXBOX_PAD_RUMBLE` (1): SetRumbleState drives XInput motors; `TUXBOX_GI_ACCEPT_CALLBACKS` (1): reading/system-button/layout callbacks return S_OK with a token; `TUXBOX_GI_DEVICE_HISTORY` (1): next/previous readings walk each device's own last 32 (VIctory-fxceless gameinput.rs, GPL-3.0, credit).

### d3d12: barriers and lists

- `TUXBOX_DS_GENERIC_READ` (0): 1 keeps DIRECT_QUEUE_GENERIC_READ on depth.
- `TUXBOX_DS_READ_SR` (0): 1 keeps SHADER_RESOURCE in DEPTH_STENCIL_READ.
- `TUXBOX_DS_SYNC` (1), `TUXBOX_DS_PLANE_SPLIT` (1): depth sync and per-plane barriers.
- `TUXBOX_BARRIER_LEARN` (1): 0 logs rejected barriers only.
- `TUXBOX_BARRIER_SKIP` (1): 0 records barriers no variant fixes.
- `TUXBOX_NULL_TEXTURE_BARRIER` (1): 0 refuses resource-less texture barriers.
- `TUXBOX_NULL_RTV_CLEAR` (0): 1 passes null-RTV clears on.
- `TUXBOX_PLACED_UNDEFINED` (0): 1 passes LAYOUT_UNDEFINED through.
- `TUXBOX_RECT_UNWRITTEN` (1): 0 copies every declared channel in the rectangle GS.
- `TUXBOX_INDIRECT_ORDER` (1), `TUXBOX_INDIRECT_GUARD` (1), `TUXBOX_INDIRECT_SCRATCH` (1), `TUXBOX_INDIRECT_CALL_TIME` (1): ExecuteIndirect ordering, guard, scratch clamp, call-time clamp.
- `TUXBOX_INDIRECT_STALE` (run clamped): `empty` empties every stale call.
- `TUXBOX_INDIRECT_STALE_NARROW` (1; 0 every call after a skip stays stale): a skipped dispatch whose writes are bounded to root UAVs leaves stale only ExecuteIndirects whose argument or count buffer holds one of those addresses. A dispatch refused on a paged descriptor arena names its compute pipeline and root shape once; the dropped-dispatch log names the pending pipeline key.
- `TUXBOX_INDIRECT_MAX` (65536): MaxCommandCount cap.
- `TUXBOX_COPY_FLAGS_STRICT` (0): 1 refuses nonzero copy flags.
- `TUXBOX_SCISSOR_CLAMP` (1): 0 passes over 16 scissors.
- `TUXBOX_COPY_SCRATCH_MB` (64): same-buffer copy scratch, 0 off.
- `TUXBOX_ALLOC_SLOT9_OK` (1): 0 answers E_NOTIMPL.
- `TUXBOX_STRICT_LISTS` (0), `TUXBOX_STRICT_SCENE` (0): 1 drops the whole batch on a failed list or BLAS.
- `TUXBOX_ORPHAN_WRITES` (1): 0 skips CPU writes for left-out lists.
- `TUXBOX_ORPHAN_WRITES_MAX` (waited words): `all` every word, `0` none.
- `TUXBOX_OVERLAP_STRICT` (0), `TUXBOX_PAGE_MAPPING_STRICT` (0): 1 drops overlapping or refused batches.
- `TUXBOX_SLOT35_DIRECT_ONLY` (0), `TUXBOX_QSLOT35` (drop): slot-35 submissions.
- `TUXBOX_FENCE_CELL_MAX` (1): 0 stores the fence cell as read.
- `TUXBOX_UPLOAD_PER_QUEUE` (1), `TUXBOX_PLACED_CROSS_WAITS` (1): placed upload queues and cross-queue placed-texture fence waits; 0 turns each off.
- `TUXBOX_SPLIT_AT_WAITS` (1): 0 skips waits after rendering work.
- `TUXBOX_SPLIT_CAP` (uncapped): caps list splits.
- `TUXBOX_WAIT_HOIST` (1), `TUXBOX_WAIT_SKIP_BARRIER` (1): unsplittable waits hoisted; skipped waits get a UAV barrier.
- `TUXBOX_QUAD_MASK_REFUSE` (0): 1 refuses quad sample masks.
- `TUXBOX_FORGET_RELEASED_LISTS` (1): 0 keeps a released list's recording.
- `TUXBOX_DEVICE11_SLOT59` (1): 0 maps slot 59 to ExecuteIndirect, not the InsertGpuMarker no-op.
- `TUXBOX_LEASE_ON_COMPLETE` (1): table leases retire when every submission of their list completes on the GPU; 0 holds until reset (VIctory-fxceless draw.rs, credit).

### d3d12: GPU memory waits and presentation

- `TUXBOX_WAIT_WRITER_RELEASE` (1), `TUXBOX_WAIT_MAP_STRICT` (0), `TUXBOX_WAIT_SAFE_READ` (1).
- `TUXBOX_WAIT_DEAD_MS` (100): short wait after a watchdog release, 0 off; `TUXBOX_WAIT_DEAD_STICKY` (1).
- `TUXBOX_MEMORY_WAIT_FORCE` (1): release unwritten held waits at present; `TUXBOX_PRESENT_RELEASE_ALL` (1): every held wait is released at each present (five-star MemoryWaits.ReleaseAll); 0 releases only waits no queued list writes.
- `TUXBOX_THROTTLE_RELEASE` (1): 0 doesn't release held waits at the throttle escape.
- `TUXBOX_PRESENT_LAG` (2): presents a queue may lag, 0 off; `TUXBOX_PRESENT_WAIT_MS` (5000); `TUXBOX_PRESENT_HELD_MS` (250).
- `TUXBOX_PRESENT_TYPELESS` (1): 0 refuses typeless present surfaces.
- `TUXBOX_RETIRE_WAITS_ALL` (0): 1 waits on every queue before completion writes.
- `TUXBOX_PRESENT_BGRX` (1): 0 refuses B8G8R8X8 presents (drawn into BGRA with alpha 1).
- `TUXBOX_WAIT_TITLE_WORD` (1): a wait whose word's buffer cannot be mapped reads the word at the title address, and marker/value writes to it are made by the CPU there once their batch has run; 0 drops such waits.
- `TUXBOX_WRITER_DONE_FRESH` (1; 0 off): a resubmitted list drops its done value until this run's progress signal, so a GPU memory wait is not released early on the previous run's completion.

### d3d12: resources, views and imports

- `TUXBOX_ROOT_FLAGS` (1), `TUXBOX_ROOT_CHECK` (1): root-signature flag clearing and checking.
- `TUXBOX_COMMITTED_HEAP_STATE` (1): upload/readback initial states.
- `TUXBOX_RGB9E5_TARGETS` (1), `TUXBOX_PACKED_ALIAS` (1): RGB9E5 targets as half floats; packed-light alias.
- `TUXBOX_DEPTH_CLEAR_CLAMP` (0), `TUXBOX_DEPTH_DEFAULT_SRV` (1), `TUXBOX_MAP_HEAP_CHECK` (1).
- `TUXBOX_DEPTH_TWINS` (1): full depth/stencil UAV twins; 0 off.
- `TUXBOX_ROOT_SPAN_REGIONS` (1), `TUXBOX_NULL_ROOT_SINK` (1), `TUXBOX_REMAPPED_CBV_COPY` (1), `TUXBOX_CBV_CLAMP_END` (1).
- `TUXBOX_HOST_COPIES` (1): placed and viewed title buffers are host copies filled before each submission, writable ones merged back after the fence (no title range opened as a heap); 0 imports by address.
- `TUXBOX_SHADOW_COVER` (1): a view placed inside a live host copy's range (256-byte offset, matching writability) reuses that copy, one copy per title buffer (ztrx/VIctory victory-gpu shadow.rs, GPL-3.0, credit); 0 a copy per view. test172 without it: 6120 live copies, 4338 MiB rising ~60 MiB/s, GPU removed at t=77.75 s from memory exhaustion, 1.9 fps, submit 384-803 ms.
- `TUXBOX_WATCH_CLIP_CACHE` (1): the write watch clip caches each VirtualQuery region 1 s; 0 queries every range.
- `TUXBOX_WRITEBACK_IMAGE_GUARD` (1): write-backs and writable host copies never land in a loaded module image (refusals logged once per image); 0 off.
- `TUXBOX_ZTRX_SLOTS` (1): DispatchMeshX (older #98 / newer #100) runs DispatchMesh(x,1,1) and ResetX at older #137, per ztrx list.rs; 0 makes DispatchMeshX a no-op. `TUXBOX_SLOT_TRACE` (1): first-call trace of list slots 59..148 per table (list, two args, game+rva); 0 off.
- `TUXBOX_ROOT_STALE_CLEAR` (drop): `sink` draws with zeros, `0` refuses.
- `TUXBOX_ROOT_COMMIT_PAGE` (0): 1 commits only the first page of a root view.
- `TUXBOX_ROOT_VIEW_TABLES` (1): root SRV/UAVs are one-view tables; 0 off. `TUXBOX_VIEW_TABLE_REBIND` (1): a tabled parameter is never bound as a root view (test157's boot hang) and tabled views are rewritten and rebound after a heap change; 0 restores the old fallback. test159 (c50a2d4, every default on): no GPU hang.
- `TUXBOX_LOW_VSHARP_EMPTY` (1): a guest buffer view below 4 GiB that is none of ours reads as empty; 0 imports it (rebox 18c049c).
- `TUXBOX_ROOT_VIEW_END` (1): a root view running past its lazy view takes its span from xmem; 0 walks VirtualQuery.
- `TUXBOX_ROOT_RAW_STRIDE` (1): `tb_root_raw_stride` root SRV of guest root raw view strides; 0 off.
- `TUXBOX_BVH_RAW_SRV` (1): acceleration-structure SRVs bind as raw SRVs over the title memory (heap and root forms); 0 off.
- `TUXBOX_ROOTCONST_PAD4` (1): compute root constants rounded up to whole 16-byte rows; 0 off.
- `TUXBOX_TABLE_ARENA_FREE` (1): table arena slots are released (VIctory HostHeap); 0 never frees.
- `TUXBOX_DS_UAV_DROP` (1): a depth-stencil texture with unordered access is created without it in every layout and dimension (leonida 89fdb83e0); 0 keeps the bit.
- `TUXBOX_INNER_DEPTH_OWN` (1): a depth image placed inside another's range gets its own render heap (leonida d16eb1ff0); 0 aliases.
- `TUXBOX_PLACED_LAYOUT_MAP` (1): console layout words (0x100+) forward as UNKNOWN / ROW_MAJOR; 0 forwards them raw.
- `TUXBOX_PACKED_ALIAS_COPY` (1): an R9G9B9E5 texture over a UAV-only R32_UINT twin is copied from it before a read; 0 reads title memory.
- `TUXBOX_PLACED_DEVICE` (1): a 64 KiB-aligned buffer the title places in its GPU page pool (slot 44 or 133) gets a DEFAULT-heap buffer, CPU touches fault its pages open and they are copied up before each submit (LMIII/VItoriaBr/leonida, credit); turn-downs log why; 0 imports as before.
- `TUXBOX_ROOT_WALK_CACHE` (1): a root view's committed span is kept 1 s so binds inside it skip VirtualQuery; 0 walks every bind. `TUXBOX_ROOT_SPAN_IMPORT` (1): a walked root view imports the 32 MiB-aligned window of its span (up to 64 MiB) so nearby binds reuse it (test167: 1285 ms per 10 s on 64 KiB imports); 0 imports 64 KiB.
- `TUXBOX_SLOT_WORDS` (1): `tb_slot_words` (t6, space32766) holds 32 bytes of console GFX10 words (V#/T#) per heap slot; 0 leaves it out. `TUXBOX_TABLE_WORDS` (1): `tb_tw{p}` (space32765) is a raw root SRV over the title memory at root parameter p, or the table's encoded slot words for a descriptor table; 0 leaves the bindings out.
- `TUXBOX_MODE0_64K` (1): a 2D layout-0x100 texture every model refuses is sized as the console's 64 KiB chain (ScarlettCompat b92f060); 0 refuses. `TUXBOX_ALLOC_UAV_RETRY` (1): a description refused only for ALLOW_UNORDERED_ACCESS is sized without the bit (ScarlettCompat e6588c4); 0 keeps the refusal.
- `TUXBOX_ADDRESS_UAV` (1), `TUXBOX_ADDRESS_UAV_CHECKS` (1), `TUXBOX_UAV_IMPORTS` (1), `TUXBOX_UAV_REIMPORT` (1).
- `TUXBOX_TYPED_UAV_GUARD` (1), `TUXBOX_OFF_ELEMENT_RAW` (1), `TUXBOX_HEAP_NULL_TAIL` (1).
- `TUXBOX_SLOT_VIEWS` (1), `TUXBOX_VIEW_MARKS` (1): slot_views table and write marks.
- `TUXBOX_JOIN_RERESERVE` (1), `TUXBOX_JOIN_AHEAD_MB` (32), `TUXBOX_JOIN_SKIP_PINNED` (1).
- `TUXBOX_SHADOW_IMPORT_CACHE` (1), `TUXBOX_SHADOW_COPY_REUSE` (1).
- `TUXBOX_IMPORT_KEEP` (512), `TUXBOX_IMPORT_REUSE` (1), `TUXBOX_IMPORT_SINGLE` (1), `TUXBOX_IMPORT_SWEEP` (1), `TUXBOX_IMPORT_PRECHECK` (1), `TUXBOX_IMPORT_RETIRE` (1).
- `TUXBOX_MEM_DIR` (1; 0 off): a page directory over the 40-bit title address space for translated compute stages (MESIA-Unified memory.rs / draw.rs, GPL-3.0, credit): t0 space32764 a root SRV of 2^21 {copy, start} entries, one per 64 KiB page; u0 a 4096-word fault UAV ((page % 4095) + 1, 0 the sink); b0 the heap index of a 1024-slot raw-UAV copy run past each shader-visible heap's root-table ring (slot 0 null). Copies are the title imports with unordered access; each dispatch makes ready its root view addresses, 40-bit root-constant pairs and constants >= 0x01000000 as <<8 addresses (plus the next 4 MiB), and faults are read at most every 16 ms. Stages are recognised by a PSV0 binding at t0/space32764; paged-arena heaps get an empty directory. A copy whose import the registry dropped (swept every 250 ms) or whose title range was remapped leaves the directory: its pages go back to empty (readers fault, then see the new import), its resource is kept 2 s for lists in flight, then its number and run slot are reused.
- `TUXBOX_SLOW_IMPORT_LINE` (1; 0 off): each OpenExistingHeapFromAddress1 over 20 ms logs size, time and private bytes.
- `TUXBOX_IMPORT_REMAPPABLE` (remapped ranges kept out): `1` opens every view, `mapped` keeps out all physical views.
- `TUXBOX_REFUSED_PAGES` (1), `TUXBOX_RESERVATION_WALK_ALL` (0), `TUXBOX_PUBLISH_SCAN_ALL` (0), `TUXBOX_HOST_FLAG_MASK` (1).
- `TUXBOX_SHADOW_CPU` (1), `TUXBOX_SHADOW_FILL_CHECKED` (1).
- `TUXBOX_SHADOW_HASH_COMMITTED` (bits): `query` uses VirtualQuery, `0` reads directly.
- `TUXBOX_SHADOW_HASH_LOCKED` (0), `TUXBOX_SHADOW_DROP_LOCKED` (0), `TUXBOX_SHADOW_PRUNE_EVERY` (0), `TUXBOX_SHADOW_RELEASE_ON_REMAP` (1).
- `TUXBOX_SHADOW_HASH_MB` (4): a shadow refresh hashes at most this many MiB of aged checks, the rest owed and due next refresh; 0 hashes all. `TUXBOX_SHADOW_SKIP` (1): unchanged-shadow hash skip; 0 off.
- `TUXBOX_WATCH_HANDOFF_ONCE` (1): the write watch merges hand-off pages once per refresh by binary search; 0 merges per scan. `TUXBOX_WATCH_OWE_WATCHED` (1): a reset page is owed to the other side only inside a range it watched; 0 owes every page.
- `TUXBOX_SHADOW_RECENT` (4): a writable shadow is written back and refreshed whole only while written in the last N write-backs; 0 keeps the sticky mark. test159: loading screen at 143 fps, submit 0.8 ms, shadow copies near 0.
- `TUXBOX_SHADOW_WATCH` (1): unwritten read-only shadow pieces are skipped, and the watch turns itself off after a refresh that watches nothing or takes over 2 ms; 0 copies them all.
- `TUXBOX_REFRESH_VRAM_SCALE` (1): the refresh read budget scales with the device-local budget (ScarlettPort f14d3ff); 0 keeps the fixed budget.
- `TUXBOX_EVICT_PRESSURE_TIERS` (1): span eviction idle time shortens under device-local pressure (VItoriaBr vram_idle_ms); 0 off.
- `TUXBOX_HOLD_FROM_SUBMIT` (1): a list's eviction-hold age counts from its last submit; 0 from its recording's start. `TUXBOX_TARGET_RETOUCH` (1): recordings retouch their RTV/DSV heaps at each submit instead of the hold; 0 keeps the hold.
- `TUXBOX_EVICT_BLOCKER_LINE` (1): an over-budget pass that evicts nothing names its blocker every 10 s; 0 off.
- `TUXBOX_SLOT108_FALLBACK` (1), `TUXBOX_BACK_IMAGES` (0).
- `TUXBOX_RESERVED` (1), `TUXBOX_PLACED_BACKSTOP` (1).
- `TUXBOX_LAYOUT0_BY_KIND` (1), `TUXBOX_ALIAS_EXACT` (1), `TUXBOX_PLANE_SIZE_MORE` (1), `TUXBOX_FMT10_PITCH` (1), `TUXBOX_PACKED_3D_OFF_PAGES` (1).
- `TUXBOX_SLOT_HOLDS_TEXTURE` (1), `TUXBOX_PART_VIEWS_STAY` (1), `TUXBOX_MESH_TOPOLOGY_UNDEFINED` (1).
- `TUXBOX_BCPACK_VICTORY` (1), `TUXBOX_BCPACK_BC7_ENTRY0` (1); `TUXBOX_BCPACK` (grey): `refuse` refuses bad streams.
- `TUXBOX_HEAP_FAIL_LOG` (1).
- `TUXBOX_IDENTITY_GUARD` (1): 0 gives unpaged heaps no null guard slot.
- `TUXBOX_PACKED_READ_NOTE` (1): R9G9B9E5 read notes (unpublished write, zero memory).
- `TUXBOX_LOW_VSHARP_EMPTY` (1): an address SRV over a low V# none of ours binds a null view; 0 imports.
- `TUXBOX_PACKED_ALIAS_ANY` (1): packed alias copies serve any GPU-written UAV/RT non-depth owner of the same shape; 0 only R9G9B9E5 over UAV R32_UINT.
- `TUXBOX_RAW_STRUCTURED` (1; 0 off): structured SRVs and counterless UAVs are created raw (R32_TYPELESS), paired with xshader `TB_IDXEN_RAW`.
- `TUXBOX_RAW_BIAS` (1; 0 off): a structured window not starting on 16 bytes is made raw from the rounded-down start, its byte bias (0..15) in bits 24-31 of the slot_views stride word; only slots rewritten raw carry it.
- `TUXBOX_MESH_HOISTED_SRV` (1; 0 off): a guest table the mesh root replaces with a Descriptor (FSSM 0x100) becomes a root SRV bound per draw to the buffer in the table's slot 0.
- `TUXBOX_SRV_AS_UAV` (1; 0 off): SRV slots a program stores to (`<key>.slots`/`.heap` uav_from_srv lines) get a one-descriptor UAV range and a private table copy with a UAV of the resource; typed-UAV-family colour textures are made ALLOW_UNORDERED_ACCESS, anything else gets a null UAV and a line.
- `TUXBOX_SHADOW_REFRESH_MB` (64; 0 off): each shadow refresh copies at most this, owing the rest oldest-first (test172 copied 268 MB at once, device removed 7.5 s later).
- `TUXBOX_SHADOW_HASH_DIRTY_ONLY` (1): a read-only shadow whose pages the write watch says are unwritten keeps its hash without hashing; aged checks still hash.
- `TUXBOX_SHADOW_FILL_THREADS` (4; 1 off): CPU fills of 4 MiB or more split over threads.
- `TUXBOX_SHADOW_CENSUS` (1): at most every 30 s, live shadow copies bucketed by refreshes since last bound.
- `TUXBOX_SHADOW_UAV_REMARK` (1): each draw/dispatch re-marks the shadows behind its root signature's UAV table ranges so they keep being written back; re-mark lines log at lists #1,2,4,...
- `TUXBOX_WATCH_REFUSED_CACHE` (1): a placed-texture write-watch range the OS refused is not asked again for 10 s (test173: 6167 refusals in 10 s, most of upload.rs's 9-11 ms per submit).
- `TUXBOX_SHADOW_READ_COMMIT` (1; 0 commits the whole range first): a CPU piece reads lazy reserved pages without committing; write-back commits only changed pages.
- `TUXBOX_SHADOW_SLAB` (1; 0 one buffer each): CPU pieces up to 64 KiB share 4 MiB upload slabs (test177: 60,157 pieces took ~3.7 GiB commit for 541 MiB).
- `TUXBOX_SHADOW_EVICT_STALE` now also sees heap slots read by literal/root-constant index and dynamic heaps indexed per lane or via root CBV.

### d3d12: page pools, placed textures and write-back

- `TUXBOX_PAGES_RECYCLE` (4096), `TUXBOX_PAGES_RECYCLE_MB` (256, 0 count only), `TUXBOX_PAGES_RECYCLE_SECS` (10, 0 never), `TUXBOX_PAGES_SIGNAL_EACH` (0).
- `TUXBOX_PAGES_EVICT` (0), `TUXBOX_SPAN_EVICT_PCT`, `TUXBOX_SPAN_EVICT_S` (10), `TUXBOX_PAGES_SYSTEM_PCT` (off), `TUXBOX_PAGES_DEMOTE` (1).
- `TUXBOX_PAGES_TRIM_EMPTY` (1), `TUXBOX_PAGES_PACK` (1), `TUXBOX_PAGES_TRIM_PCT` (95), `TUXBOX_PAGES_TRIM2_PCT` (110).
- `TUXBOX_PAGES_IDLE_S` (5), `TUXBOX_PAGES_IDLE_MIN_KB` (1024), `TUXBOX_PAGES_TILED_CHECK` (1), `TUXBOX_PAGES_CONTIGUOUS` (1).
- `TUXBOX_RENDER_HEAP_PRIORITY` (1), `TUXBOX_RENDER_HEAP_EVICT` (0).
- `TUXBOX_COMMITTED_WRITEBACK` (1): committed-texture write-back on CPU touch; 0 off.
- `TUXBOX_CLAIM_SAME_KEEP` (1).
- `TUXBOX_SNAPSHOT_BIG_MB` (16), `TUXBOX_SNAPSHOT_BIG_MS` (500), `TUXBOX_SNAPSHOT_MAX_MB` (256), `TUXBOX_WRITEBACK_MAX_MB` (on).
- `TUXBOX_REFRESH_READ_MB` (32), `TUXBOX_REFRESH_READ_MS` (4), `TUXBOX_REFRESH_ROTATE` (1), `TUXBOX_DEFER_OWED` (1), `TUXBOX_REFRESH_BUSY_SCALE` (3).
- `TUXBOX_PLANE_UNBUDGETED` (1), `TUXBOX_BINK_PLANE_GATE` (1), `TUXBOX_BINK_FRAME_GATE` (1), `TUXBOX_BINK_PLANES_READ` (1), `TUXBOX_BINK_CALL_TRACE` (0).
- `TUXBOX_WRITEBACK_INFLIGHT_MB` (512), `TUXBOX_READERS_MERGE` (1).
- `TUXBOX_PLACED_OOM_RETRY` (1), `TUXBOX_PLACED_OOM_STRICT` (0), `TUXBOX_PUBLICATION_STRICT` (0).
- `TUXBOX_WATCH_GAP_MB` (64).
- `TUXBOX_DETILE_FAST` (1): 0 copies placed uploads run by run. test153: submit fell from 27 ms to about 1 ms per frame; 39 fps until a hang at present #2699.
- `TUXBOX_PLANE_DECODE_SKIP` (1): a video plane already read since Bink's last completed decode is skipped, once that count has moved; 0 reads every plane every refresh.
- `TUXBOX_UPLOAD_WORKERS` (1 = inline), `TUXBOX_UPLOAD_LIST_POOL` (1): detile worker threads; 0 creates copy lists per refresh.
- `TUXBOX_PAGES_COMPACT` (1): over budget, once a second, the sparsest idle pool's pages move to other pools so trim_empty can evict it; moves wait on every title queue's fence first.
- `TUXBOX_PAGES_COMPACT_FILL_PCT` (50; was 25): pools up to this full are compacted; when none qualifies each live pool's first blocker is logged every 30 s.
- `TUXBOX_PAGES_DENSE_FIRST` (1): a texture no pool fits whole takes tiles from the densest pool first; 0 the pool with most free tiles.
- `TUXBOX_PAGES_RELEASE_EMPTY` (1; 0 evicts as before): empty page pools over budget are released, one kept.
- `TUXBOX_READBACK_POOL_MB` (256; 0 off): placed-texture write-back readback buffers pooled by power-of-two size and reused (test177 made 5931, 15019 MiB).

### xmem

- The 15 s map-calls stats line also logs the live pool: pages allocated, pages mapped, addresses mapped, pages at 2+ addresses.

- `TUXBOX_LAZY_COMMIT` (1), `TUXBOX_LAZY_ALLOWS` (1), `TUXBOX_COMMIT_BITS` (1).
- `TUXBOX_READ_COMMIT` (1), `TUXBOX_READ_COMMIT_CRT` (1).
- `TUXBOX_XMEM_UNMAP_FALLBACK` (1), `TUXBOX_XMEM_DEFER_UNMAP` (1), `TUXBOX_XMEM_UNMAP_RETRY` (0).
- `TUXBOX_XMEM_DEVICE_ALLOC` (1), `TUXBOX_XMEM_GRAPHICS_BUDGET` (1).
- `TUXBOX_JOIN87_DETAIL` (1): failed join unmaps name the GPU imports over them.
- `TUXBOX_UNMAP87_REGION` (1): an unmap failing 87 logs the view's region, neighbours, working-set flags and file reads in flight into it; 0 off.
- `TUXBOX_JOIN_FREEZE` (0; 1 on): a join freezes the other threads only for a retry after it failed 87 unfrozen (xbox-emulator phys_runs.rs, credit); 0 off. test166 (freeze on every join) hung in boot state 1 with 0 presents from the freeze cost; 4adc117 fixes it.
- `TUXBOX_JOIN_FREEZE_GIVEUP` (1): joins stop freezing after 3 frozen retries in a row fail 87; 0 keeps freezing. No frozen retry has cured an 87 (test168, test171).
- `TUXBOX_POOL_SECTION_IMPORT` (0), `TUXBOX_POOL_SECTION_COMMIT` (0): 1 hands d3d12 title sections (TuxBoxPageSection) for OpenExistingHeapFromFileMapping imports, as SEC_COMMIT roots; off so eager joins run, since the 6236 MiB pool as SEC_COMMIT overcharges commit.
- `TUXBOX_WATCH_JOURNAL` (1): write watch keeps one journal of each 64 KiB block's last write and per-reader read marks (ztrx victory-gpu watch.rs, credit); blocks read in full by both readers are dropped. 0 keeps the owed sets. `TUXBOX_WATCH_STATS` (1): write watch line every 10 s per side (queries, MiB, pages, lock/query time, refused queries); 0 off.
- `TUXBOX_BINK_BUF_WATCH` (0x5d0000000): each Bink open logs the allocations its HBINK points into and this page with its newest unmap; =0 off.
- `TUXBOX_XMEM_UNMAP_TRACE` (1): ring of the last 4096 unmapped ranges (TuxBoxUnmapTrace); 0 records nothing.
- `TUXBOX_SECTION_WATCH` (1): write-watch queries the OS refuses on title section views are answered by guarding 64 KiB blocks read-only; the first write faults, is noted and unguarded. The guard lock is held per block only and the write handler waits for it (794137f/90d96e0, test174's crash). Kernel write ranges stay unguarded 2 s.
- `TUXBOX_SECTION_WATCH_RUNS` (1; 0 off): every committed read-write run of a block is guarded, and blocks with nothing committed are not reported (test177: 302748 of 370256 were). A census logs at queries #1,2,4,... and every 4096.

### d3d12: pipelines

- `TUXBOX_SLOT57_REAL` (0), `TUXBOX_SLOT67_REAL` (0): 1 builds slot 57 state-block records and slot 67 console compute PSOs for real; off they answer E_NOTIMPL as ztrx does (test170 with REAL on was black at 144 fps). `TUXBOX_SLOT57` (real): `placeholder` answers slot 67 with a placeholder; `TUXBOX_SLOT67_AS57` (0): 1 decodes slot 67 as slot 57.
- `TUXBOX_SLOT57_REFUSED_NOTIMPL` (1): a record build() refuses answers E_NOTIMPL; 0 a placeholder. `TUXBOX_SLOT57_VARIANT_WAIT` (1): a variant in progress is waited for up to 500 ms; 0 drops its draw. `TUXBOX_SLOT57_DUMP` (0): 1 writes each record's code slots to `%TEMP%	uxbox-slot57-<hash>-s<n>.bin`.
- `TUXBOX_CB_DUAL_SOURCE_RT0` (1): RT1-7 beside a dual-source RT0 are left unblended (Xbox2Windows, MIT); 0 keeps them.
- `TUXBOX_JOURNAL_BUFFERED` (1), `TUXBOX_JOURNAL_WARM` (1): the pipeline journal reads through an 8 MiB buffer on its own thread from precache load; 0 seeks per frame / opens at the first pipeline.
- `TUXBOX_BIND_SKIP_HEAVY` (1; 0 waits): a title thread never starts or waits on a driver compile of 256 KiB of DXIL or more; it goes to the workers.
- `TUXBOX_COMPILE_LANES` (min(4, cores/4); 1 single lane): large compiles share one queue, title-bound pipelines before prefetches.
- `TUXBOX_HEAVY_COMPILE_SERIAL` (1; 0 together): 1 MiB+ compiles queue with no give-up and log queue depth. `TUXBOX_HEAVY_COMPILE_BUDGET` (1; 0 strictly one at a time): another starts while free commit covers all running peaks (1 GiB per MB of DXIL) plus 2 GiB.
- `TUXBOX_COMPILE_START_LOG` (1; 0 off): each large compile logs its start (key, KiB, thread, others running) and "still compiling" at 30 s and each doubling.
- `TUXBOX_PIPELINE_MISS_LOG` (1; 0 off): pipeline-library misses (name, HRESULT, per-record digests) and failed StorePipeline calls are logged, first 16 then powers of two.
- `TUXBOX_PIPELINE_BUILD_ALL` (0; 1 all journal pipelines): the prewarm builder makes only pipelines a run bound, most-bound first, starting none past `TUXBOX_PIPELINE_BUILD_MIN` minutes (10; set by `NEONBRIDGE_PREWARM_PSO_MIN`).
- `TUXBOX_SLOT57_ANY_SLOT` (1): a missing VS/PS is taken from any code slot whose FSSM header names the stage; 0 reads slots 0 and 1 only.
- `TUXBOX_PIPELINE_THREAD_CAP` (1): compile threads capped to a quarter of the cores; 0 keeps half plus a quarter.
- `TUXBOX_COMPILE_LANE` (1): driver compiles of 256 KiB DXIL or more run on one long-lived lane thread; 0 compiles on the calling worker.
- `TUXBOX_THROTTLE_RELEASE_BOUND` (1): an fps-floor-held worker takes a job the title has bound; 0 holds it.
- `TUXBOX_SHADER_KEY_LOCK` (1): the cache publish lock is per digest; 0 uses the 32 buckets.
- `TUXBOX_MESH_STATIC_SAMPLER` (1), `TUXBOX_MESH_TYPED_BUFFER` (1): mesh bindings read static samplers and four-element typed buffers (TuxBox 63343eb); 0 turns each off.
- `TUXBOX_PIPELINE_WORKERS` (half the cores, 2-8), `TUXBOX_PIPELINE_HANDOFF_MS` (150), `TUXBOX_COMPILE_WORKERS` (2), `TUXBOX_PREFETCH_WORKERS` (quarter of cores, up to 4).
- `TUXBOX_PIPELINE_SHAPE` (unset): `fxceless` drops prefetchers and the fps hold.
- `TUXBOX_PATIENT_URGENT` (1; 0 off): a ready pipeline worker's translation request is marked urgent like a title thread's, so the launcher serves it ahead of the background prewarm.
- `TUXBOX_HEAP_LATE` (1; 0 creation's lookup only): a pipeline whose stage `.heap` was missing at creation looks it up again at its binds, at most once a second, until found.
- `TUXBOX_COMPILE_FRESH_THREAD` (1; 0 off): compiles of 1 MiB DXIL or more run on a thread made for that one compile, which exits after it (the driver keeps its arena with the compiling thread); after the first four, falls back to the lanes unless private bytes fell by half on exit. `TUXBOX_HEAVY_ALONE_WAIT_S` (120): a heavy compile running alone still waits for its own peak plus the margin, at most this long.
- `TUXBOX_TITLE_NO_JOIN` (1; 0 joins): a title thread (or steal) never joins a pipeline compiling on another thread; the job goes to the workers first in line and its binds skip until made.
- `TUXBOX_STORE_DEFER` (1; 0 waits): a pipeline-library store that meets a save serialising is left to the next save, which stores it before serialising. `TUXBOX_SAVE_SNAPSHOT` (1; 0 off): a library save serialises into memory under the store lock and writes the file after releasing it. `TUXBOX_SAVE_LOG` (1; 0 off): each save logs its MiB, ms holding the stores and ms writing after.
- `TUXBOX_PIPELINE_FPS_FLOOR` (30): 0 never holds workers.
- `TUXBOX_PIPELINE_PRIORITY` (1): 0 no bound-first queue; `normal` keeps workers at normal priority.
- `TUXBOX_PIPELINE_HURRY` (1), `TUXBOX_EAGER_ON_TITLE` (0), `TUXBOX_GRAPHICS_FIRST` (1), `TUXBOX_DISPLAY_BUILD_NOW` (1).
- `TUXBOX_MESH_LINK` (1), `TUXBOX_MESH_LINK_FIRST` (1), `TUXBOX_MESH_TOPOLOGY` (1), `TUXBOX_CB_TARGET_MASK` (1).
- `TUXBOX_PIPELINE_CACHE_SHARED` (1), `TUXBOX_PIPELINE_LIBRARY_MB` (16384), `TUXBOX_PIPELINE_LIBRARY_MAPPED` (1), `TUXBOX_PIPELINE_LIBRARY_HANDLE` (1), `TUXBOX_PIPELINE_LIBRARY_NEXT` (1), `TUXBOX_PIPELINE_LIBRARY_TRIM` (1).
- `TUXBOX_PIPELINE_SAVE_LOCKED` (0), `TUXBOX_PIPELINE_STORE_LOCKED` (0), `TUXBOX_PIPELINE_PARALLEL_LOAD` (1).
- `TUXBOX_PIPELINE_JOURNAL` (1), `TUXBOX_PIPELINE_MAX_KB` (8192), `TUXBOX_PIPELINE_CENSUS` (1), `TUXBOX_PIPELINE_FAILURES` (1).
- `TUXBOX_PIPELINE_WAIT_MS` (500; 60 s before first present), `TUXBOX_FRAME_WAIT_MS` (50), `TUXBOX_PENDING_RECHECK` (1).
- `TUXBOX_SMALL_COMPUTE_BYTES` (16384), `TUXBOX_FIRST_USE_BUDGET` (1), `TUXBOX_BIND_WAIT_BUDGET` (1), `TUXBOX_INLINE_UNCACHED` (0).
- `TUXBOX_COMPUTE_PENDING_WAIT_MS` (3000), `TUXBOX_COMPUTE_PENDING_FRAME_MS` (4000), `TUXBOX_COMPUTE_STEAL` (1), `TUXBOX_COMPUTE_OVER_BUDGET_MS` (750), `TUXBOX_DISPLAY_DRAW_WAIT_MS` (250).
- `TUXBOX_REFUSED_PLACEHOLDER` (1), `TUXBOX_UNTRANSLATABLE_BINDS` (32).
- `TUXBOX_DERIVED_PSO` (apply): `base` or `fail`; `TUXBOX_DERIVED_PSO_MB` (512).
- `TUXBOX_SHADER_LOCK_SKIP` (1).
- `TUXBOX_COMPILE_BUDGET` (prefetch only): `all` gates every compile, `0` none; `TUXBOX_COMPILE_BUDGET_WAIT_MS` (30000).
- `TUXBOX_COMPILE_COMMIT_RESERVE` (program-sized): `fixed` keeps 8 GB.
- `TUXBOX_COMPILE_ESTIMATE_FIT` (1): 0 uses 4096 B per DXIL byte, 2-32 GB.
- `TUXBOX_MAX_DRIVER_COMPILES` (4), `TUXBOX_COMPILE_CEILING_GB` (28), `TUXBOX_COMPILE_CEILING_WC` (0), `TUXBOX_COMPILE_CAP_ALL` (1).
- `TUXBOX_COMPILE_COMMIT_FLOOR_MB` (3072), `TUXBOX_FLOOR_HOLDS_WANTED` (0), `TUXBOX_WANTED_AT_ONCE` (6), `TUXBOX_BUDGET_RECHECK` (1).
- `TUXBOX_COMPILE_MEASURE` (1).
- `TUXBOX_RUNTIME_SHADERS` (1): 0 runs without shaders.
- `TUXBOX_PREWARM` (0): 1 adds all of `shaders.rpf` to `--prewarm`.
- `TUXBOX_COMPUTE_EAGER` (1): every title compute pipeline goes to the workers at creation, not first bind.
- `TUXBOX_DISPATCH_PENDING_WAIT` (1): a dispatch whose pipeline is pending waits once more (bounded) instead of being dropped.
- `TUXBOX_JOURNAL_FLUSH_S` (1; 0 only at library saves): the pipeline journal reaches the file within N s of each new pipeline.
- `TUXBOX_QUAD_MASK_VARIANT` (1): a draw under a partial SetSampleMaskX mask binds its pipeline's quad-mask variant (PS as <digest>_qm.quadmask); 0 full coverage.
- `TUXBOX_PLACEHOLDER_REASONS` (1): every placeholder pipeline logs its refusal reason once at first bind; 0 keeps the 32 cap.
- `TUXBOX_REFUSED_WAIT_LOG` (1): a refused shader a title thread waits on logs 'title waits on refused shader <sha>: <reason>' once per key.
- `TUXBOX_PSO_RELEASE_IDLE` (0; 1 on): a lazy pipeline unbound `TUXBOX_PSO_IDLE_S` (120) gives back its native pipeline and is remade at next bind; freed `TUXBOX_PSO_RETIRE_S` (10) later.
- `--prewarm` builds PSOs with a runtime from the tree (or `--prebuilt`), not the staged d3d12_xs.dll; `NEONBRIDGE_PREWARM_STAGED_DLL=1` loads the staged copy. Linked VS/PS `.heap` and `.slots` sidecars are published by key and producer digest.

### d3d12: samplers, ray tracing

- `TUXBOX_SAMPLER_FIVESTAR` (1): slot 61 samplers decode as five-star ConsoleSamplerDescription; 0 decodes as fxceless. `TUXBOX_SAMPLER_DECODE` (fxceless): `victory` keeps the old decode.
- `TUXBOX_SAMPLER_FOLD` (1): 0 off.
- `TUXBOX_SAMPLER_SANITIZE` (1), `TUXBOX_ANISO_POINT_MIP` (1), `TUXBOX_INTEGER_TARGET_BLEND` (0), `TUXBOX_SMEM_SIGNED_OFFSET` (s_load only: `all` or `0`).
- `TUXBOX_SAMPLER_PAIR_MOVES` (1), `TUXBOX_PACKED_STORES` (1).
- `TUXBOX_FOREIGN_INSTANCES` (1), `TUXBOX_AS_NULL_SKIP` (1), `TUXBOX_AS_BUILDS` (1), `TUXBOX_AS_TRACE` (0), `TUXBOX_RAYS` (1).

### DirectStorage, game runtime, audio

- `TUXBOX_EOF_PARTIAL` (0), `TUXBOX_XCRD_CHECK_DEST` (1), `TUXBOX_XCRD_REFUSED_RECORD` (1).
- `TUXBOX_DSTORAGE_PAGE_WAIT_MS` (8), `TUXBOX_DSTORAGE_RETRIES` (12), `TUXBOX_DSTORAGE_RETRY_PLAIN` (0), `TUXBOX_DSTORAGE_PAGES_LIVE` (1), `TUXBOX_STORAGE_FAILURE_CAUSES` (1), `TUXBOX_ZLIB_CHUNK_STEP` (1).
- `TUXBOX_TEXT_ENTRY` (ask): set to answer without asking; `TUXBOX_TEXT_ENTRY_DIALOG` (1).
- `TUXBOX_SPEECH` (silent): `refuse` refuses the synthesizer.
- `TUXBOX_XASYNC_EARLY_CALLBACK` (1), `TUXBOX_XERROR_CALLBACK` (1), `TUXBOX_XERROR_NULL_MESSAGE` (0).
- `TUXBOX_SAVE_READ_LOCK` (1), `TUXBOX_SAVE_QUOTA_FIXED` (0).
- `TUXBOX_CHAD_STRICT` (0), `TUXBOX_XMA` (1), `TUXBOX_XMA_LOOP_SKIP_NEXT` (1), `TUXBOX_APU_FREE` (1), `TUXBOX_AUDIO_LOCK_PATCH` (1).
- `TUXBOX_XAPU_CLAMP` (1), `TUXBOX_XAPU_REBOX_LAYOUT` (0), `TUXBOX_XAUDIO_VOICE_FLAGS` (1).
- `TUXBOX_RUNTIME_FEATURE` (1): XGameRuntimeIsFeatureAvailable is served (leonida); 0 answers E_NOINTERFACE.
- `TUXBOX_BINK_SOUND_STATS` (1): wraps BINKSND Ready/Lock/Unlock (+0x170..+0x180), logs first calls, a 10 s line of rates, the title's queue and mixer state, and failed sound opens; 0 off. test167 (JOIN_FREEZE=0) reached the loading screen (741 presents), then crashed at game+0x1a9b33f under the Bink sound open_wrapper (null +0x638) at the player-select A press; 305 render heaps, 2 freed; the shader worker's run folders were missing.
- `TUXBOX_XAPU_EAGER` (1): lenient XAPU answers (rebox, re-derived): a Process keeps up to frame_count frames, an undecodable packet gives silence; 0 strict. `TUXBOX_SPATIAL_LEVEL` (1): every 10 s, spatial objects' buffer peaks (periods over -60 dB) and volumes; 0 off.
- `TUXBOX_BINK_LEAD` (1), `TUXBOX_BINK_CATCHUP` (0; 4 restores the cap: test171 spent ~150 ms/s on it), `TUXBOX_BINK_WAIT_CATCHUP` (1), `TUXBOX_BINK_PLANE_FENCE_MS` (8), `TUXBOX_BINK_SKIP_SIGNAL` (1).
- Saves (VIctory-fxceless xgamesave.rs, GPL-3.0, credit): `TUXBOX_SAVE_KEEP_RESULT` (1) keeps a too-small read's result for the retry; `TUXBOX_SAVE_MISSING_NOT_FOUND` (1) BLOB_NOT_FOUND for never-written containers; `TUXBOX_SAVE_UPDATE_LIMIT` (1) E_GS_UPDATE_TOO_BIG over 16 MiB; `TUXBOX_SAVE_GS_ERRORS` (1) E_GS_OUT_OF_LOCAL_STORAGE / E_GS_NO_ACCESS on I/O failures.

### Diagnostics
- `TUXBOX_COMMIT_BY_CALLER` (1; 0 off): lazy commits per caller logged at most every 10 s. The subrender trace logs group-event SetEvents and waits on all 128 group events; the mesh hoisted SRV logs its first 8 binds.

- `TUXBOX_FPS` (1), `TUXBOX_FRAME_WORK` (1), `TUXBOX_WINDOW_REPORTS` (1), `TUXBOX_TITLE_CLOSE_LOG` (1), `TUXBOX_CRASH_RECORD` (1).
- `TUXBOX_TRACE_PRESENT` (launcher sets 120): sample every Nth present.
- `TUXBOX_CAPTURE_FRAME` (off), `TUXBOX_CAPTURE_FRAMES` (1), `TUXBOX_CAPTURE_DIR` (`%TEMP%\tuxbox_capture`), `TUXBOX_CAPTURE_SHADERS` (0).
- `TUXBOX_HANG_PEEK` (off): `off:len,...` image offsets to dump at a hang.
- `TUXBOX_HANG_WAITS` (1), `TUXBOX_FENCE_STALLS` (1), `TUXBOX_QUEUE_WAITS` (1).
- `TUXBOX_WAIT_STACKS_MS` (1500), `TUXBOX_WAIT_STACKS` (3).
- `TUXBOX_NULL_VA_TRACE` (1), `TUXBOX_PLANE_TRACE` (1), `TUXBOX_SAME_MEMORY_WATCH` (1).
- `TUXBOX_SIZE_WATCH`, `TUXBOX_TRACE_ALLOCATION_BYTES` (off): size-query traces.
- `TUXBOX_TRANSIENT_WATCH` (0), `TUXBOX_TRANSIENT_WATCH_MS` (50), `TUXBOX_TRANSIENT_POOL_RVA` (0x75df1b8).
- `TUXBOX_GPU_SEGMENTS` (0): GPU timing of title submit segments.
- `TUXBOX_LIST_RING` (0), `TUXBOX_BARRIER_PROBE` (0), `TUXBOX_BARRIER_RING` (1).
- `TUXBOX_FALLBACK` (off): `flat`; `TUXBOX_NULL_SHADERS`, `TUXBOX_NULL_BISECT` (off): swap shaders out to find a bad one.
- `TUXBOX_BREAK_RVAS` (off): log registers at these RVAs.
- `TUXBOX_DRED` (0).
- `TUXBOX_FENCE_WAIT_LOG_MS` (2000): log title fence waits held longer, 0 off.
- `TUXBOX_ROOT_BRANCHES` (1): root_address binds by branch and ms every 10 s; 0 off.
- `TUXBOX_HANG_FIND` (0x23cf104,0x43c3d5d,0x43c421e): hang-time group waits and Subrender workers; 0 off. `TUXBOX_SUBRENDER_DUMP` (1): a 5 s heartbeat stall dumps the loading-screen Subrender handshake and GPU channel once; 0 off.
- `TUXBOX_WATCH_HANG` (1): 100 ms in-process sampler of the title's main state, loading flag and heartbeat (five-star --watch-hang); 0 off.
- `TUXBOX_WRITE_WATCH` (ring credits, game+0x75e5ed8): counts writes from the taker 0x2a42d0 and logs every other writer RIP up to 4 times with its game returns; 0 off. test159 named game+0x24825a7 as the ring-credit releaser, and ended at 119 s on an int3 at game+0x14a6390.
- A write watch on a game+RVA past the exe image is refused, and `TUXBOX_SUBRENDER_TRACE` is not installed when the image is too small (the prewarm builder loads d3d12_xs into neonbridge.exe). The shadow UAV re-mark logs a heap-reflection census and notes heap writers run with no resource heap bound.
- `TUXBOX_HANG_SECONDS_BOOT` (30): a stall before the first present prints every thread's stack once after N s; 0 off.
- `TUXBOX_LOG_TIME` (1): every stderr line ends with `[t=S s]` since process creation; 0 off.
- `TUXBOX_COMMIT_WALK_EVERY` (6): the commit breakdown walk runs every Nth memory line or after 256 MiB of commit movement; 1 walks every line. The residency line counts render heaps freed; the root views line counts new buffer imports.
- `TUXBOX_D3D12_SDK` (off): Agility SDK dir for a debug device; `TUXBOX_D3D12_SDK_GLOBAL` (0), `TUXBOX_D3D12_SDK_VERSION` (from D3D12Core.dll).
- `TUXBOX_SUBRENDER_TRACE` (1): title-image thread exits, Subrender worker tids at their first go wait and job SetEvents (first 64, then powers of two).
- Shadow refresh/write-back lines carry totals since start; shadow fill lines give CPU pieces, bytes and time; TuxBoxPageWhy names why a title page is unreadable.

### xshader (`TB_*`)

Every set `TB_*` switch is part of the shader cache key. `TB_NO_WAVESIZE_<n>` is set from the device's wave range.

- `TB_COMPACT` (1): compact dispatch for repeated blocks.
- `TB_RAW_IDXEN` (1), `TB_RAW_IDXEN_LOOSE` (0), `TB_IDXEN_STRIDE_GUARD` (0).
- `TB_HEAP_LOAD_STRICT` (0), `TB_HEAP_CLAMP_ALL` (1), `TB_PIXEL_HEAP_ROOTED` (1).
- `TB_SOPP_26_NOP` (0), `TB_DIV_OPS` (1), `TB_IMAGE_ATOMIC_EXT` (1), `TB_CUBE_UAV` (1), `TB_PK_INT16_MORE` (1), `TB_SAD` (1), `TB_CVT_F32_F64` (1), `TB_VOP1_F64` (1), `TB_BITREPLICATE` (1), `TB_DS_CMPST` (1).
- `TB_FF1_FMT69` (1), `TB_WIDE_ATOMIC_ANY_PS` (1), `TB_CVT_F16_OMOD` (1), `TB_MAD_UNFUSED` (1), `TB_MAD_FTZ` (1), `TB_TYPED_HALF4` (1), `TB_TYPED_UNALIGNED` (1), `TB_TYPED_NAN_BITS` (1), `TB_NORM_EXACT_ENDS` (1), `TB_INT32_IMM` (1).
- `TB_WRITELANE_CARRY` (1), `TB_MIMG_ADDR_LIVE` (1), `TB_READLANE_VECTOR` (1), `TB_GROUP_READ_SGPR` (1), `TB_DPP_QUAD_ANY_WAVE` (1), `TB_PARTIAL_WAVE` (1), `TB_EXEC_HALF_CONST` (1).
- `TB_MASK_HIGH_SHADOW` (1), `TB_MASK_CFG` (1), `TB_MASK_UNSPILL` (1), `TB_MASK_BOTH_FORMS` (1), `TB_MIXED_MASK` (1), `TB_BCNT_EXEC_WAVE` (1), `TB_FF1_EXEC_WAVE` (1), `TB_LANE_ELECTION` (1), `TB_INLINE_SAMPLER_WORDS` (1).
- `TB_SCC_VOTE` (1), `TB_VCC_VOTE_EXEC` (0).
- `TB_RESINFO_DIMS` (1), `TB_RESINFO_CUBE` (1), `TB_PCK_SNORM` (1), `TB_F64_ARITH` (1), `TB_CVT_SATURATE` (1), `TB_DERIV_STRICT` (0).
- `TB_CMP_LEVEL_APPROX` (0), `TB_MEMTIME_ZERO` (0), `TB_DESC_AS_DATA` (0), `TB_DESC_DATA_EXACT` (1): a descriptor read as data counts only when its tag is known on every path; 0 counts every tag.
- `TB_EXP_VM_LAST` (1), `TB_EXP_VM_START` (0).
- `TB_FOLD_NULL_BRANCH` (1), `TB_SETPC_ADDC` (1), `TB_INLINE_EXIT_STUBS` (1), `TB_ENDPGM_BRANCH_EXIT` (1), `TB_SKIP_FLAG_BITS` (1), `TB_MOVE_BLOCKS` (1), `TB_STRUCTURED_BOUND` (1).
- `TB_EXEC_GROUP` (1), `TB_EXEC_GROUP_COMPUTE` (1), `TB_EXEC_GROUP_HELPERS` (1).
- `TB_LOOP_CAP` (4096): `stage` picks 128/256/4096, or a number; `TB_LOOP_PER_COUNTER` (1).
- `TB_FENCE_GUARDED` (1), `TB_FENCE_CAPPED` (0).
- `TB_SPLIT_WAVE64` (1), `TB_WAVE64_FOLD` (1), `TB_WAVE64_SPIN` (1).
- `TB_MESH_DROP_UNREAD` (1).
- `TB_QUAD_MASK` (0): 1 adds the SetSampleMaskX quad mask prologue to pixel shaders.
- `TB_VIEW_MARKS` (1), `TB_BVH_RAW` (1): exact BVH node tests over raw views (five-star rdna_bvh.hlsli), `TB_BVH_ROOT` (1): root T#s too; `TB_BVH_NO_HIT` (1): unresolved BVH reads miss instead of refusing. 0 turns each off.
- `TB_WAVE64_MASK_LIVENESS` (1), `TB_SPLIT_WAVE64_MULTI` (1): split wave64 for 65..1024-thread groups; `TB_MASK_DATA_WAVE` (1): a mask read as a number pins wave64; `TB_LIFT_GUARDED_BARRIER` (1). 0 turns each off.
- `TB_HEAP_VIEW_WORDS` (1): heap buffer loads get dwords 1-2 from `tb_slot_views`, only when read as data (`TB_HEAP_VIEW_WORDS_ALWAYS` (0): 1 after every load); `TB_RAW_IDXEN_STORE` (1), `TB_ROOT_RAW_STRIDE` (1). 0 turns each off.
- `TB_TABLE_WORDS` (1): a root-table descriptor read as data loads the guest's words from `tb_tw<t>`; 0 reads the stale SGPRs. `TB_DESC_DATA_ARITY` (1): the descriptor-as-data check reads only the sources an op uses; 0 reads all three. `TB_WAVE_LOOP_QUAD_SWIZZLE` (1): a QDMode ds_swizzle_b32 in a compute loop reads its quad lane with WaveReadLaneAt; 0 refuses.
- `TB_CFG_SPLIT` (unset: DPP quad-exchange programs only): xcl make_reducible node splitting; 1 all interleaved programs, 0 off.
- `TB_TABLE_ARRAYS` (1), `TB_VGPR_HEAP_DESC` (1), `TB_EXECZ_INLINE_WORDS` (1), `TB_DF_STICKY_CONFLICT` (1), `TB_CB_MUBUF_SOFFSET` (1), `TB_D16_INT_IMAGE` (1), `TB_BPERMUTE_UNIFORM_SCC` (1), `TB_WIDE_IMAGE_ATOMICS` (1), `TB_WAVE_LOOP_SWIZZLE` (1), `TB_MESH_LOOPS` (1): 0 restores each old refusal or behavior.
- `TB_STRUCTURED_CF` (1): programs with interleaved loops go through cfg_split and are emitted structured, not through the tb_pc dispatcher.
- `TB_GUARD_MERGE` (1): in bodies over 256 KiB, consecutive one-line statements under the same EXEC guard share one if block.
- `TB_PIXEL_SAMPLER_HEAP_ROOTED` (1): a rooted PS under SAMPLER_HEAP_DIRECTLY_INDEXED takes its sampler heap base from the FSSM class-6 SGPR, also when the root names a heap register.
- `TB_HEAP_REG_FSSM` (1): a compute program's heap register is its FSSM class-5 SGPR, not the root-word count (537591f, the test173 hang).
- `TB_COMPUTE_FSSM_TABLE` (1): a compute root rebuilt from FSSM names each table descriptor by its FSSM row, not the guest RTS0 range (1ed6594a).
- `TB_IDXEN_RAW` (1; 0 off): IDXEN heap buffer accesses read a ByteAddressBuffer at index * slot stride + byte (ztrx 578d459, credit). `TB_SLOT_BIAS` (1; 0 reads the word as stride): raw slot reads add the stride word's bias in bits 24-31.
- `TB_SRV_AS_UAV` (1; 0 off): a store/atomic to an RTS0 SRV slot declares it a UAV and emits a uav_from_srv line. `TUXBOX_MESH_TABLE_AS_RAW` (1; 0 off): a mesh FSSM raw-buffer record on a table parameter rebinds it as that root SRV.
- `TB_SRV_AS_UAV_BUF` (1; 0 off): buffer_store/atomics through a root SRV slot retype it UAV. `TB_SRV_AS_UAV_READ` (1; 0 refuses): reads of a retyped slot use the same RWTexture.
- `TB_SAMPLER_HEAP_FSSM` (1; 0 off): a pixel sampler at root-constant index << 4 on the FSSM class-6 pointer is a heap sampler even off 9 + num32.
- `TB_F32_NATIVE_CMP` (1; 0 off): in programs of `TB_NATIVE_CMP_MIN_INSNS` (8000)+ instructions, flushed f32 compares are native with NaN tested on the bits.
- `TB_MEM_DIR` (1; 0 keeps the zero stand-ins): getpc-less flat loads and BVH s_loads in compute read title memory by guest address through the runtime page directory (t0/u0/b0 space32764, `TUXBOX_MEM_DIR`) via `tb_mem_load`; a missing page records a fault and reads zeros; such programs compile as cs_6_6 (MESIA-Unified emit/lower.rs memory(), GPL-3.0, credit). `TB_BVH_FLAT_ZERO` (1; 0 refuses): a getpc-less flat load in a BVH program reads zeros (no hit) where `TB_MEM_DIR` does not apply. `TB_WAVE_LOOP_GROUP_READ` (1; 0 refuses): a group lane read inside a guest loop keeps its group exchange when the group is exactly one 64-lane guest wave (all branches scalar, so every invocation meets each barrier).

## Recent runs

- test173: about 18 fps; hung at 367 s on a refused compute shader whose heap register was wrong (fixed by 537591f).
- test174: crashed at 46.8 s on the TUXBOX_SECTION_WATCH guard lock race (fixed by 794137f/90d96e0).
- test175 (TUXBOX_SECTION_WATCH=0): 23-26 fps, then a Subrender group-2 deadlock at a scene change at 254 s (open); device-local over budget from 218 s.
- test177: about 11 fps, reached the Lucia loading screen, hung at 410 s: Subrender group 2 never got a job (worker 1 set group 3). Free commit min 343 MiB. 81k pipelines still created lazily (prewarm loaded a stale DLL; fixed by 81b8e5e). Shadow UAV re-mark hit 0 views; SRV_AS_UAV logged nothing.
- test178 (rev 3fa508f): 897 s, 16450 presents. 18-22 fps behind the Lucia art from t=101 (in gameplay, no reads after 96 s); no Subrender hang. About 1/3 of dispatches dropped as "pipeline still being made" while 1.1-1.3 MB compiles ran serially (addressed by the heavy-compile budget). At 825 s device-local went 979 MiB over budget and texture write-backs blocked on WRITEBACK_INFLIGHT; 3-4 s submits followed and run.sh stopped it as slow. Readback pool reused 8190 of 8192.

## Credits

Neonbridge stands on other people's work. GPL-3.0 and MIT code is used with credit, and so is code from the GPL-2.0-only projects xcl and rebox. "as" names our commit. Thank you to:

- **[TuxBox](https://github.com/brcly/TuxBox)** by brcly (Apache-2.0): the console-DLL providers, D3D12 runtime and `xshader`, vendored under `vendor/tuxbox`. Upstream fixes ported in: reserved resources, tile commands and xmem updates (aeed457, as cf8ae9f, 68bb418, 0a87554); raw IDXEN byte offsets (242121b, as dd50685); the title-assert step-over; one-dword raw buffer transfers; packed 16-bit integer ops and the clamp bit (fae402f, 34ba13a); `s_ff1_i32_b64` and MTBUF format 69 (d575b34); wide image atomics in every pixel shader (2a7fd48); `s_mov_b64` sampler pair copies (4cd8d1c); list slots shared by both Xbox generations (ClearState, desktop copies, resolve, bundles, predication, stream output, debug markers; 00f69b8, as 8c8ecd6); the missing-DXC warning (02dcc15, PR #52 by Valdi320, as b31979c); list slot 61 as ExecuteIndirect and slot 101 DriverHintX (bedf503, as 4355c0e); by Valdi320, the live-import sweep and per-caller import failure log (97642c7, 56cbb05, as 555455e) and allocation-info answer sizes (be20365, as e97674f); DispatchMesh and DXR list slots (b6647ec, as 720dd5b); placed buffers imported after commit keep their title address (6171f56 by Valdi320, as 91731fc); the idea of a root CBV window across adjoining regions (5c73bee, as 3669bd2); stale imports retired on remap, from its queue-ordered remap check (45c7395 by Valdi320, as 0ac09be); the shader translator merged up to 7b815f2, including 63343eb, bd7108c, f80dff3 and de5639f (by brcly, JohnVonWells and contributors, as 61c1359); straight-line DPP8 exchanges (7d7dc72 by Valdi320, as 2afc171); mesh vertex output sized to dxc's 32 KiB output budget (f111f5c by brcly, as b5bd633); heap descriptor origins through control flow (1181af3 by Valdi320, as f86556f); portable pixel consumer component types (6a2ca92 by Valdi320, as a45f98c); exact DPP8 extrema, XOR and FI exchanges (7e76da5 by Valdi320, as fbd6d97); wave sums over forward execz skips (d59da4d by Pavel Kirilin, as 2061aa4), exec groups in every stage (ab06fe1 by absceptual, as 9deb8ed) and whole-wave group reads for SGPR-lane readlanes (4774547 by Pavel Kirilin, as 0159df2); int32 immediates (5891be3, as 80399f6) and rooted pixel heap bases (0b6e788, as 7e596bb); the address UAV re-import (ff2cdfb, as 2b08914), host flag masking (9ba53f9, as 1a832ad) and the strict raw IDXEN refusal of 63343eb made default (as 820ed05); the skipped-wait barrier and ignored quad sample mask (3f2a717, as ad8e641).
- **[VIctory](https://github.com/Marvisak/VIctory)** by Marvisak: typed null descriptors, async pipeline building, argument clamps, per-frame pipeline budgets, the fine timer for the audio mixer, the list of what the menus need (UI runtime, relaunch, low memory placement, commit), the frame-capture idea (`VICTORY_DUMP_TARGETS`), and, under GPL-3.0, sharing one texture memory across formats, so far as the watch (13bbd6f, as 625552e); the mesh topology record that lets libraries hold mesh pipelines (9c58a1c, as 4e749f4); XAsync firing the routine of the call that finished (d01b170, as ba792df); the GPU timeout log (42961e8, as 259fca6), sampler value checks (43b7c08, PR #474, as f9e90cd), anisotropic point-mip samplers (170ab23, PR #1337, as b3cf1df) and parallel library loads (b00c14a, PR #1000, as eccb6d3); XMA ring wraps (7a5d85e and 1054fe8, PR #536, as f0f92ec); DirectStorage page-list re-reads (d09f3aa, PR #1726, as b789893) and the clamp skip for dropped indirect calls (4e60ea3, PR #1635, as 4b28545); the barrier-in-divergent-flow finding (e9765a7, as fd04a6a); tbuffer d16 loads (46bde7f, as da066d0), the static sampler comparison mode (2286e03, as 0d40b0f) and half interpolation modifiers (4436fca, PR #1696, as 4b6ad63); readfirstlane with no active lane (5fa5de3, as ce0763f); folding a branch to the next instruction (825c045 by EatCodeSleep2, as 95260ba); `s_quadmask` and constant-mask `v_cndmask` (95f6c6e and 6160c9e, as 407854f); logging a failed first-touch commit (`xmem.rs`, as 7877eb6) and the 13.5 GiB title memory budget (`console.rs`, as 04a7bee); layout 0 by kind (texture_layout, as 50d7fa0) and UNDEFINED mesh topology (host_topology, as c0a1faa); exact normalized ends (645391b, as d1b9283), byte-wise `chad` zeroing (643fc6d, as 42115cf), the XMA loop skip (985d8d4, as 040cb0d) the source-keyed shader cache (c47b529, as cb5737f) and refusing SOPP 0x26 (79fb722, as a86ac3f); the null descriptor tail (host.rs:510-590, as ad6ecbe and 255359c), the watchdog call skip (hooks.rs, as d528561).
- **[VIctory (LMIII fork)](https://github.com/LMIII98/LMIII)** by LMIII98: the idea of forcing lighter graphics settings before each run, since the title writes its own back on exit (progress notes §27, as d2da417).
- **[VIctory (zhaibingye fork)](https://github.com/zhaibingye/VIctory)** by zhaibingye (GPL-3.0): NaN payloads kept in typed buffer loads (f876a82, as 2afc171); constant 64-bit compares (d7536bd, c012f2d by Davi713, as 4aee46b); the typed UAV format guard (ca168de, dbd4fc2, as 4a7be03); no blending on integer render targets (3716dd5, as 5ce18c3); the rectangle GS leaving unwritten channels unread (a64b6c6, as bb6d002).
- **[victory-gta6](https://github.com/fxceless/victory-gta6)** by fxceless (GPL-3.0): the idea of LOD 0 for implicit-derivative samples outside pixel shaders (578d459, as 2afc171); RGB9E5 targets as half floats, through the VIctory-1984 fork (3a55abd, as 8b13e6e); the double-precision add form (3bff182, as 78ce93c); the mapped library's open file handle (578d459, as ad53891); DirectStorage store retries (578d459, as b789893); the view-index clamp idea (31714de, as 8b14d05); the 500 ms per-frame pipeline wait (`make.rs`, as 74ea0e3) and re-checking a pending pipeline at each draw (`draw.rs`, as d240ffa); running indirect calls with clamped records after a dropped dispatch (`indirect.rs`, as 28aee37); library saves that don't block loads, the `.next` save and working-set trim (`library.rs`, `host.rs`, as 8807bc8 and 0c1db65); the program-sized prefetch reserve (as 3504dcc); half the cores for pipeline workers (`make.rs`, as 94b6ada); splitting lists at every memory wait (`list.rs`, `queue.rs`, as 7d7963e); the busy refresh budget (`draw.rs`, as 92823b8); per-stage loop caps and the 0 read for an unmatched f16 interpolation half (`lower.rs`, through ztrx and VItoriaBr, as b9bcd25); loop counters per header (lower.rs 10906-10918, as 58558ec), lane-mask unspill pairing (ir.rs 309-380, as 26c98c4), refusing `image_sample_c_l/_b/_d` (lower.rs 9430-9440, as bb076bd) and `s_memtime` (lower.rs 5573, as 45822bc); shared-lock pipeline stores (library.rs:556-612, as 8bc2d9c), the XAPU record layout (apu.rs:384, as bb85084) the page-list-only store retry (as df46da9); the pipeline shape switch (VIctory-fxceless make.rs:295-300, as 449d9f7) and text entry asking (9d94a1c, as 74e5372); the packed-light alias (`alias_of`, as 5e3a1ee), waits hoisted to the list start (7d7963e, as b44fceb) and, copied with credit, depth twins (victory-gpu resource.rs:322-570, as 00b029c and 000f78a).
- **[xcl](https://github.com/ivall/xcl)** by ivall (GPL-2.0-only): page-split copies, zero-filled placed uploads, triple-buffered presentation, the resize and memory-wait designs, unknown slots answering, the finest timer resolution for choppy audio, XAudio2 creation clean-up and network blocking. Committing title memory on first touch is ported from `kernelx/lazy.rs` (38b05b7, with 77905f6 by Logan) as 49702da; wave64 compute as two wave32 halves (3ecf4cf, 77a5eee, e40ac38, as 57ff63d); the BCPack modes 3 to 6 decoder `bcpack_victory` (itself from VIctory's `victory-formats` bcpack.rs, as e9d9996), graphics-first pipeline builds (`pso_async.rs` f14d3ff, as a12339a) and the indirect guard (`bind.rs` 0223c29, cc73c11, as 6607576), all taken through ScarlettPort; the offline build's DXIL size cap (cbff0b8 and 6b9b14f, with WebIdentity1, as 495546b); the commit floor growing with the program (28946bb, as 2332ba8); mixed mask and number scalar operations (438d420 by Kalmarv, as e7f6be4); constant and NULL mask operands (15fff3b, as 1ebaf02); skipping VirtualQuery on committed windows (c1a9b52, as b6c7088); resinfo on 1D, 3D and array views (cafda82, as 078b76a); refusing gather4h and gather8h (61ce2f9 by sennecools, as 4378100); refusing GDS DS instructions (e642921, as a2672da); the sleep hold (`kernelx.rs`, as 650790a); VCC branch votes (1c971de, as b381815); the title memory cap (`config.rs`, as 04a7bee) and hooking the CRT's ReadFile (`vfs_hooks.rs`, as 81a0a3a); the last VM=1 export deciding pixel survival (dc0d25f, as 4799d57); the timed-out write-back park (52272a0, PR #578, as 45a5d72) and the page-array read without faulting (db6d692, PR #509, as 336feef); `s_ff1` of a mask (f90033c); waited-on compiles skipping the queue (9a272d4, as 0bbda8d), the budget re-check (724b9e2, as b1061ae) and floor holds sparing waited-on compiles (724b9e2, as 491a300), in-process dxcompiler.dll compiles (`gpu/recomp/dxc.rs`, as 570e0aa), packed-mip 3D textures off pages (971c0e5, as 942be6f), released lists forgetting their recording (b55c478, as 78ed617), BC7 entry 0 (7881dbc, as d300833), the VM=1 start state (dc0d25f, as 9f9ef0c), cube resinfo (get_resinfo, as 77d28ba), XAudio2 voice flags (hle/xaudio2.rs, as 092580e) and, copied with credit, bounded wave64 half meets (066ae5b, as d7fe510).
  - Its 64 KiB video-memory page pool for placed textures, with superseded-texture unmapping and remapping (`unmap_superseded`, `remap_if_unmapped`, the 2 s idle and cool-down delays, the mapping fence before submits), is copied from `crates/xcl/src/hle/d3d12x/vram.rs` (commit 7695539, from ad79932/38b05b7; the insert/remove rules of `title_textures.rs`) into `vendor/tuxbox/runtime/d3d12/src/device/placement/pages.rs`, which keeps xcl's credit and GPL-2.0-only header. The tiled-support check is copied there too (24d8b92 with 38ea451), and so is keeping a texture's tiles in one heap (3bc7143 by raylitebull, as 700bba2). Also ported: ExecuteIndirect waiting for its argument writes (9486494 by wondercrash, as 56d4d5e) and never gating a compile a game thread waits on (724b9e2, with five-star-recomp's "programs on demand", as 7404809).
  - Re-derived: the default depth SRV (615cb20), the Map heap check (21de1a8), marking reserved resources (cf244b0, 7d834d4), keeping refused DirectStorage reads (66660bd, 2e17d3d), same-buffer copies through scratch (21bc548, 392dc0c), raising a waited-on pipeline worker (25c6f7e, 9a272d4), merging write-watch scans (712bbbe). Ported: the structured-output bound (69a892c, as 692983f).
- **[Xbox2Windows](https://github.com/R0Y00/Xbox2Windows)** by R0Y00: sampler-heap bounds, shader-translation cross-checks, scalar bit-count instructions, flat integer parameters, the derived-pipeline blend patches, and typeless surfaces presenting (99624a8d); signed no-carry add/sub (4fe59c25, a lead); the idea of a refusal census (5510deca, b4d1cf01, 425cd53b, as 2afc171); DPP16 float source modifiers (after d8b51b5f by KomaGuy, as 5580795) and scalar bit tests (7ea49908, ca1ed1a9, as 71bdee8); the shift-add SCC (d2f0e1df, 85a2aa65, as b48e766); `s_ff1` of a tracked scalar (a81a6f07, as 20d4332); partial colour export masks (03dee932 and 27f32934, as 8b14d05); SDWA conversion modifiers (ad3d5126, as da066d0) and half `v_fma_mix` inline constants (c1e83dd6 and dc9ba9ed, as 0d40b0f); clearing lane-mask marks on multi-dword scalar writes (7ebe57e7, as 11635ed); refusing depth compares with explicit LOD (d21e3d6e, as bb076bd); shared save read locks (5550ec2b, as ff5dfa4), more console plane sizes (a6879558, as bbeb504) and the chunked zlib step (bb35f045, 4f67e704, as ad6e5a7); refusing malformed root signatures per request (155397e6, as e588824).
  - Ported under MIT: clearing an unresolved root view (109e7dc8, as ea4abef); the coalescing `ApuFree` (64b6d2d3, f9033a9b); packed RGBA8 image stores (b898d318); SAD lowerings (87e48ad7, PR #1656 by rm-NoobInCoding, as 6494bcb); `v_cvt_f32_f64` (d6624c0e, PR #1660 by JB2WIN, as 10d0f37); VOP1 opcodes from its disassembler (1ca92d57, PR #1661 by Vlad Bondarenko, lowered as b9aea60); output modifiers before the clamp (ceccbcae, d7346d7e, PR #1707 by triggered02, as 54c2dce); `s_bitreplicate_b64_b32` (285cbc6c, PR #1582 by Asimmimx, as 5c8ec41); unfused multiply-adds (f7b7297d, PR #1768 by Jess-Doit, as 3258c66); failed DirectStorage requests by cause (0ad04b91, 8c77885b, PR #1781 by MasterDomino, as 812fae7); upload and readback heap states (3556bc28 by lukaslw-lab, as 4cfbd32); unaligned typed buffer loads (5d0dce30, as 774a080).
  - Re-derived under MIT: format 71 half-float loads (5d0dce30, PR #1699 by triggered02); the wave-level `s_bcnt1` of `exec_lo` (b7854f13, PR #1872 by henriazz10, as 14752ad); the wave-level `s_ff1` of `exec_lo` (07c262a4 by henriazz10, as 2f3a49a).
- **[xbox-emulator](https://github.com/omonkeyz/xbox-emulator)** by omonkeyz (GPL-3.0): fence handling on device removal, network blocking, upload-buffer recycling and the 48 kHz mastering rate. Copied with credit headers: span aliasing for placed textures, from `crates/compat/d3d12_xs/src/alias.rs` (commit 27122f7) into `vendor/tuxbox/runtime/d3d12/src/device/placement/alias.rs`; denormal-flushing multiply-adds (tombox-xshader `tb_madz`/`tb_madlz`, as 3012dc0); console core affinity, from `crates/compat/compat-common/src/os_host/affinity.rs` into `vendor/tuxbox/runtime/xmem/src/affinity.rs`.
- **[OpenScarlett](https://github.com/californiaAlmonds/OpenScarlett)** by californiaAlmonds: rendering fixes and the game-save layouts.
- **[OpenScarlett](https://github.com/OpenCommunityEdition/OpenScarlett)** by OpenCommunityEdition (AGPL-3.0): the idea of tiered pipeline pools (f8f5b79, e301ed9, re-derived as ac966c7) and of a free-commit guard (3da6373, as 733f135); no code copied; the title memory cap (`xmem.cpp`, as 04a7bee).
- **[rebox](https://github.com/puffnfresh/rebox)** by puffnfresh (GPL-2.0-only): display-sized draws waited for (8af34bf, re-derived as 2be3430), stalled memory-wait diagnostics and the 2 s watchdog, CPU copies into memory the GPU can't reach, dropping `DispatchX`, and network blocking. Re-derived: logging a full descriptor table area (f516dfb, as b882173), waiting on a busy shader-compiler lock (4b37f0d), failing storage requests into unwritable memory (f61bbc8), remembering refused import ranges (19a69d0), bounding the reservation walk (90b6578, a85c353), reading a view through its CPU address (55f1d49, as 7febc2e), slots holding placed textures (7745cef, as 3d25a79), part views not superseding (fe7f1d4, as ddd8995), live page-array reads (502959e, as 68737d0); the title memory cap (`xmem.c`, as 04a7bee); the XAPU frame clamp (`xapu.c:222`, as ad79b24); releasing a held memory wait no queued list writes at the next present (a1ebac2, as 79dff41) and building a display-sized draw's pipeline at once (c78d9f4, f0bcabe, as cc799cd).
- **[ScarlettPort](https://github.com/ZigZagBoi1/ScarlettPort)** by ZigZagBoi1: the VIctory and xcl stacks the BCPack decoder, graphics-first builds and indirect guard were taken from; the stall watchdog switch (bb3a27a, as f87fc64).
- **[ScarlettCompat](https://github.com/SSTGxf/ScarlettCompat)** by SSTGxf (MIT): the idea of clamping and rounding CBV sizes and writing a bufferless CBV as null (2ffcb48, as 9ccbb3e).
- **[palmetto](https://github.com/luizwidmer/palmetto)** by luizwidmer: queue slot 35 as clock calibration and per-lane wave tests in `xshader`. Under GPL-3.0: the conservative slot-108 size (cc7e044b); answering unmapped create-like slots for real (804a01f0, which led to the address UAV in 0173b3f); the rising fence cell (77d2f6bb); slot 35 as the console's submit (22555b24); the resource-less texture barrier as a global barrier with null-RTV clears skipped (cf825083, as ea405c0); derived pipelines applying their patches (b00248ca, as cefdfdb); naming the objects a hung thread waits on (122075ee, written in Rust as 2779d20).
- **[leonida-ce](https://github.com/IAmNotSiob/leonida-ce)** by IAmNotSiob: which extended list options and timestamp writes are safe to ignore or emulate, and the game-save shim layouts. Under GPL-3.0: the audio-lock patch (62ad4fdf), the descriptor-heap failure log (1403e90e), the `v_div_*` semantics, its GPU fixture's `image_atomic` ops (367d91ef), cube UAV stores (0d27fdcf), the allocator slot 9 answer (6e02c937, as e6a08f0), the placeholder for a refused pipeline create (83bf1f60, as 00f5205), and the format 0x0a pitch (e8ca18c79, as b7e0af4).
- **ztrx**: `extrq`/`insertq` high-quadword zeroing (console_insn.rs, as 951afd2) and the remaining save quota (xgamesave.rs:393, as fb25f3f); under GPL-3.0, the text entry dialog, copied with credit (xgameui.rs:147, as 74e5372), and subclassing the title's window procedure (lifecycle.rs, as 2dce61c).
- **[KestrelX](https://github.com/Batushn/KestrelX)** by Batushn (MIT): compiling `[WaveSize]` out per wave width, and `XErrorReport` with its callback (ee0f749) and its "" for a null message (xgameruntime.cpp:291, as c20bd63).
- **[five-star-recomp](https://github.com/ThatRecompDev/five-star-recomp)** by ThatRecompDev (GPL-3.0): the present throttle (4cc3094) and its release while a memory wait holds a queue (bd100ba, as e1ce01d); wave64 EXEC half-writes of a constant (a245f29, as 2dab147); the transient-pool watch and lifetime check (4cdbfc8, b01c2c1, as 063d001); retire waits (cf12bd9); identifying the hang watchdog (acd5260); small compute pipelines at first bind (8fa5672); the compile budget (afdd699, CompileBudget); the first-use budget (ab94e3c, FirstUseBudget.cs); RDNA range checks on indexed buffer accesses (3d287ab); the wave64 mask high-dword fix behind their story-load corruption (2781f9c, as e8f1f34); short waits on released words (efcc782, as 0217153); masks over a loop's back edge (b5fe3a7, 48c08b8, as 1c3b05c); stack dumps of stalled memory waits (ad6e007, as 91cba0e); splitting lists at memory waits (5ab7993, as c8e86fc); CBVs over remapped pages reading a copy (1ea5bce, as ab5af3f); the direct DXIL writer (being ported into xshader's `dxil` module: the LLVM 3.7 bitstream writer and container digest so far, 283bf47); and, by marquezini, the stale GPU page report (fad0b7f, PR #5, as 5958301); dropping the console copy flags (53f89d5, as 7357128); memory waits read through the kernel (c933e88, as 4d1462e); by marquezini, failed texture fills not stopping work (a654736, re-derived for the write-back latch as f1ab1be); the program cache surviving rebuilds (2d73fe7, re-derived with VIctory c47b529 as 912477b); and its normal-priority compile threads as an option (0566186, as c4f8bd8); the off-element structured view finding (955a679, as 184d486); the driver compile cap and private-bytes ceiling (9a5bcc1, as c5018ee); linking mesh producers to their pixel shader (ddcaa54, as 82055ba); the compile ceiling counting the shader worker (fec516c, as b7fdf39), the fitted compile estimate (51c338d, as 8aa01e3) and linked mesh producers dropping unread outputs (1236bfa, as 88d6e38); `ds_permute_b32` (`LdsEmitter.cs`, as 4799d57); the dispatch-loop bound (f938530, as 1befb53); the idxen `buf_limit` select rule (`rdna_prelude.hlsli`, as db4dfe1); every compile under the budget (CompileBudget, as 3959286), the ceiling without write-combined memory (4e95ab2, as cfc201d), inline first use only when cached (9d61a3c, as 1d85f07), imports following remaps (61c095b, as 94e65ff) and mask-or-integer forms (b5fe3a7/48c08b8, as 41733c2); the mesh shader linked before the create (1236bfa, as 1775f1b), released words short-waited for good (ad07dd9, as 8a74c49) and the opt-in release of every held wait at a present (b828157, as 79dff41); releasing every held memory wait at the throttle escape (bd100ba, as 812b6b9 and 2649144).
- **[GTA6-BootTest](https://github.com/OnlyMoisties/GTA6-BootTest)** by OnlyMoisties: notes on what later screens need, such as blend state from the render-state block.
- **[Leonida](https://github.com/ubiquitarianism/leonida)** by ubiquitarianism: `xshader` ports, among them `v_writelane_b32` lane liveness (f05bc538) and image address registers (7874db15); the CLZERO loop's compare flags (3b8c1555, apply_cmp_flags); legal depth clear values (673dd15d, PR #516 by sodove, as db43389); the frame-capture idea (`LEONIDA_CAPTURE`, 88797f3e). Under GPL-3.0, from `d3d12_xs.cpp`: the scissor clamp (4ff9a77f by Jan Trencansky, as 88dbc97) and the root-signature flag clearing (a5bb4b9b by tedin7, 3ff887f9 by d7ommy and 5c3487a94 by mynzjerko, as 13de1e0); the idea of console indirect type 105 as DISPATCH (c8247cdf, as d97535c); sign-bit frexp and ldexp modifiers (5f063e58 by milk4sale, as 4aee46b); the idea that a barrier into UNDEFINED keeps the layout (37c5af12 by milk4sale, as 08ae3e8); binary16 frexp (43c737f45) and `v_log_f16` (rdna2spirv).
- ~~**Leonida** by LeonidaReforge: the retail boot chain that sized the device table.~~ (no longer on GitHub)
- ~~**[leonida-anyp5](https://github.com/m3ll0wSm0ke/leonida-anyp5)**: lowering lane reads and writes to subgroup shuffles in `xshader`.~~
- **RuneLink** (the author's own OSRS client project): the game feed's design, and the `tsproxy.exe` Tailscale helper (Go, tsnet) it uses, kept in the gitignored `tools/` folder with Cloudflare's `cloudflared.exe` as the fallback tunnel.
- **Microsoft**: the public GDK headers and documentation (XGameSave, XUser, GameInput, D3D12) the slot layouts were checked against, and dxc.
