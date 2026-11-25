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
- `NEONBRIDGE_PREWARM_DXC_O0` (0): 1 prewarms at `-O0`.
- `NEONBRIDGE_STALE_SHADERS` (1): 0 waits for every stale program before starting.
- `NEONBRIDGE_URGENT_SHADERS` (1): 0 serves shaders in directory order.
- `NEONBRIDGE_DXC_URGENT_O0` (1): 0 compiles urgent shaders at `-O1`.
- `NEONBRIDGE_DXC_O1_SECS` (10): `-O1` time before the `-O0` retry.
- `NEONBRIDGE_DXC_INPROC` (1): 0 spawns `dxc.exe` for every compile.
- `NEONBRIDGE_DXC_UPGRADE` (1): 0 keeps `-O0` results.
- `NEONBRIDGE_DXC_UPGRADE_SECS` (300): upgrade compile budget.
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
- `TUXBOX_NO_WATCHDOG` (0): 1 NOPs the title's stall watchdog.
- `TUXBOX_SLEEP_HOLD` (100): shortest Sleep held during pipeline waits, ms; 0 off.
- `TUXBOX_RELAUNCH` (1): 0 ignores `XLaunchNewGame`.

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
- `TUXBOX_UPLOAD_PER_QUEUE` (1), `TUXBOX_PLACED_CROSS_WAITS` (0): placed upload queues.
- `TUXBOX_SPLIT_AT_WAITS` (1): 0 skips waits after rendering work.
- `TUXBOX_SPLIT_CAP` (uncapped): caps list splits.
- `TUXBOX_WAIT_HOIST` (1), `TUXBOX_WAIT_SKIP_BARRIER` (1): unsplittable waits hoisted; skipped waits get a UAV barrier.
- `TUXBOX_QUAD_MASK_REFUSE` (0): 1 refuses quad sample masks.
- `TUXBOX_FORGET_RELEASED_LISTS` (1): 0 keeps a released list's recording.

### d3d12: GPU memory waits and presentation

- `TUXBOX_WAIT_WRITER_RELEASE` (1), `TUXBOX_WAIT_MAP_STRICT` (0), `TUXBOX_WAIT_SAFE_READ` (1).
- `TUXBOX_WAIT_DEAD_MS` (100): short wait after a watchdog release, 0 off; `TUXBOX_WAIT_DEAD_STICKY` (1).
- `TUXBOX_MEMORY_WAIT_FORCE` (1): release unwritten held waits at present; `TUXBOX_PRESENT_RELEASE_ALL` (0).
- `TUXBOX_THROTTLE_RELEASE` (1): 0 doesn't release held waits at the throttle escape.
- `TUXBOX_PRESENT_LAG` (2): presents a queue may lag, 0 off; `TUXBOX_PRESENT_WAIT_MS` (5000); `TUXBOX_PRESENT_HELD_MS` (250).
- `TUXBOX_PRESENT_TYPELESS` (1): 0 refuses typeless present surfaces.
- `TUXBOX_RETIRE_WAITS_ALL` (0): 1 waits on every queue before completion writes.

### d3d12: resources, views and imports

- `TUXBOX_ROOT_FLAGS` (1), `TUXBOX_ROOT_CHECK` (1): root-signature flag clearing and checking.
- `TUXBOX_COMMITTED_HEAP_STATE` (1): upload/readback initial states.
- `TUXBOX_RGB9E5_TARGETS` (1), `TUXBOX_PACKED_ALIAS` (1): RGB9E5 targets as half floats; packed-light alias.
- `TUXBOX_DEPTH_CLEAR_CLAMP` (0), `TUXBOX_DEPTH_DEFAULT_SRV` (1), `TUXBOX_MAP_HEAP_CHECK` (1).
- `TUXBOX_DEPTH_TWINS` (0): 1 turns on full depth/stencil UAV twins.
- `TUXBOX_ROOT_SPAN_REGIONS` (1), `TUXBOX_NULL_ROOT_SINK` (1), `TUXBOX_REMAPPED_CBV_COPY` (1), `TUXBOX_CBV_CLAMP_END` (1).
- `TUXBOX_ROOT_STALE_CLEAR` (drop): `sink` draws with zeros, `0` refuses.
- `TUXBOX_ROOT_COMMIT_PAGE` (0): 1 commits only the first page of a root view.
- `TUXBOX_ROOT_VIEW_TABLES` (0): 1 makes root SRV/UAVs one-view tables (root signature half only).
- `TUXBOX_ADDRESS_UAV` (1), `TUXBOX_ADDRESS_UAV_CHECKS` (1), `TUXBOX_UAV_IMPORTS` (1), `TUXBOX_UAV_REIMPORT` (1).
- `TUXBOX_TYPED_UAV_GUARD` (1), `TUXBOX_OFF_ELEMENT_RAW` (1), `TUXBOX_HEAP_NULL_TAIL` (1).
- `TUXBOX_SLOT_VIEWS` (1), `TUXBOX_VIEW_MARKS` (1): slot_views table and write marks.
- `TUXBOX_JOIN_RERESERVE` (1), `TUXBOX_JOIN_AHEAD_MB` (32), `TUXBOX_JOIN_SKIP_PINNED` (1).
- `TUXBOX_SHADOW_IMPORT_CACHE` (1), `TUXBOX_SHADOW_COPY_REUSE` (1).
- `TUXBOX_IMPORT_KEEP` (512), `TUXBOX_IMPORT_REUSE` (1), `TUXBOX_IMPORT_SINGLE` (1), `TUXBOX_IMPORT_SWEEP` (1), `TUXBOX_IMPORT_PRECHECK` (1), `TUXBOX_IMPORT_RETIRE` (1).
- `TUXBOX_IMPORT_REMAPPABLE` (remapped ranges kept out): `1` opens every view, `mapped` keeps out all physical views.
- `TUXBOX_REFUSED_PAGES` (1), `TUXBOX_RESERVATION_WALK_ALL` (0), `TUXBOX_PUBLISH_SCAN_ALL` (0), `TUXBOX_HOST_FLAG_MASK` (1).
- `TUXBOX_SHADOW_CPU` (1), `TUXBOX_SHADOW_FILL_CHECKED` (1).
- `TUXBOX_SHADOW_HASH_COMMITTED` (bits): `query` uses VirtualQuery, `0` reads directly.
- `TUXBOX_SHADOW_HASH_LOCKED` (0), `TUXBOX_SHADOW_DROP_LOCKED` (0), `TUXBOX_SHADOW_PRUNE_EVERY` (0), `TUXBOX_SHADOW_RELEASE_ON_REMAP` (1).
- `TUXBOX_SHADOW_RECENT` (sticky): N sets the recent-write window.
- `TUXBOX_SLOT108_FALLBACK` (1), `TUXBOX_BACK_IMAGES` (0).
- `TUXBOX_RESERVED` (1), `TUXBOX_PLACED_BACKSTOP` (1).
- `TUXBOX_LAYOUT0_BY_KIND` (1), `TUXBOX_ALIAS_EXACT` (1), `TUXBOX_PLANE_SIZE_MORE` (1), `TUXBOX_FMT10_PITCH` (1), `TUXBOX_PACKED_3D_OFF_PAGES` (1).
- `TUXBOX_SLOT_HOLDS_TEXTURE` (1), `TUXBOX_PART_VIEWS_STAY` (1), `TUXBOX_MESH_TOPOLOGY_UNDEFINED` (1).
- `TUXBOX_BCPACK_VICTORY` (1), `TUXBOX_BCPACK_BC7_ENTRY0` (1); `TUXBOX_BCPACK` (grey): `refuse` refuses bad streams.
- `TUXBOX_HEAP_FAIL_LOG` (1).

### d3d12: page pools, placed textures and write-back

- `TUXBOX_PAGES_RECYCLE` (4096), `TUXBOX_PAGES_SIGNAL_EACH` (0).
- `TUXBOX_PAGES_EVICT` (0), `TUXBOX_SPAN_EVICT_PCT`, `TUXBOX_SPAN_EVICT_S` (10), `TUXBOX_PAGES_SYSTEM_PCT` (off), `TUXBOX_PAGES_DEMOTE` (1).
- `TUXBOX_PAGES_TRIM_EMPTY` (1), `TUXBOX_PAGES_PACK` (1), `TUXBOX_PAGES_TRIM_PCT` (95), `TUXBOX_PAGES_TRIM2_PCT` (110).
- `TUXBOX_PAGES_IDLE_S` (5), `TUXBOX_PAGES_IDLE_MIN_KB` (1024), `TUXBOX_PAGES_TILED_CHECK` (1), `TUXBOX_PAGES_CONTIGUOUS` (1).
- `TUXBOX_RENDER_HEAP_PRIORITY` (1), `TUXBOX_RENDER_HEAP_EVICT` (0).
- `TUXBOX_COMMITTED_WRITEBACK` (0): 1 enables committed write-back.
- `TUXBOX_CLAIM_SAME_KEEP` (1).
- `TUXBOX_SNAPSHOT_BIG_MB` (16), `TUXBOX_SNAPSHOT_BIG_MS` (500), `TUXBOX_SNAPSHOT_MAX_MB` (256), `TUXBOX_WRITEBACK_MAX_MB` (on).
- `TUXBOX_REFRESH_READ_MB` (32), `TUXBOX_REFRESH_READ_MS` (4), `TUXBOX_REFRESH_ROTATE` (1), `TUXBOX_DEFER_OWED` (1), `TUXBOX_REFRESH_BUSY_SCALE` (3).
- `TUXBOX_PLANE_UNBUDGETED` (1), `TUXBOX_BINK_PLANE_GATE` (1), `TUXBOX_BINK_FRAME_GATE` (1), `TUXBOX_BINK_PLANES_READ` (1), `TUXBOX_BINK_CALL_TRACE` (0).
- `TUXBOX_WRITEBACK_INFLIGHT_MB` (512), `TUXBOX_READERS_MERGE` (1).
- `TUXBOX_PLACED_OOM_RETRY` (1), `TUXBOX_PLACED_OOM_STRICT` (0), `TUXBOX_PUBLICATION_STRICT` (0).
- `TUXBOX_WATCH_GAP_MB` (64).

### xmem

- `TUXBOX_LAZY_COMMIT` (1), `TUXBOX_LAZY_ALLOWS` (1), `TUXBOX_COMMIT_BITS` (1).
- `TUXBOX_READ_COMMIT` (1), `TUXBOX_READ_COMMIT_CRT` (1).
- `TUXBOX_XMEM_UNMAP_FALLBACK` (1), `TUXBOX_XMEM_DEFER_UNMAP` (1), `TUXBOX_XMEM_UNMAP_RETRY` (0).
- `TUXBOX_XMEM_DEVICE_ALLOC` (1), `TUXBOX_XMEM_GRAPHICS_BUDGET` (1).

### d3d12: pipelines

- `TUXBOX_PIPELINE_WORKERS` (half the cores, 2-8), `TUXBOX_PIPELINE_HANDOFF_MS` (150), `TUXBOX_COMPILE_WORKERS` (2), `TUXBOX_PREFETCH_WORKERS` (quarter of cores, up to 4).
- `TUXBOX_PIPELINE_SHAPE` (unset): `fxceless` drops prefetchers and the fps hold.
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

### d3d12: samplers, ray tracing

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
- `TUXBOX_BINK_LEAD` (1), `TUXBOX_BINK_CATCHUP` (0), `TUXBOX_BINK_WAIT_CATCHUP` (1), `TUXBOX_BINK_PLANE_FENCE_MS` (8), `TUXBOX_BINK_SKIP_SIGNAL` (1).

### Diagnostics

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
- `TUXBOX_D3D12_SDK` (off): Agility SDK dir for a debug device; `TUXBOX_D3D12_SDK_GLOBAL` (0), `TUXBOX_D3D12_SDK_VERSION` (from D3D12Core.dll).

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
- `TB_CMP_LEVEL_APPROX` (0), `TB_MEMTIME_ZERO` (0), `TB_DESC_AS_DATA` (0).
- `TB_EXP_VM_LAST` (1), `TB_EXP_VM_START` (0).
- `TB_FOLD_NULL_BRANCH` (1), `TB_SETPC_ADDC` (1), `TB_INLINE_EXIT_STUBS` (1), `TB_ENDPGM_BRANCH_EXIT` (1), `TB_SKIP_FLAG_BITS` (1), `TB_MOVE_BLOCKS` (1), `TB_STRUCTURED_BOUND` (1).
- `TB_EXEC_GROUP` (1), `TB_EXEC_GROUP_COMPUTE` (1), `TB_EXEC_GROUP_HELPERS` (1).
- `TB_LOOP_CAP` (4096): `stage` picks 128/256/4096, or a number; `TB_LOOP_PER_COUNTER` (1).
- `TB_FENCE_GUARDED` (1), `TB_FENCE_CAPPED` (0).
- `TB_SPLIT_WAVE64` (1), `TB_WAVE64_FOLD` (1), `TB_WAVE64_SPIN` (1).
- `TB_MESH_DROP_UNREAD` (1).

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
