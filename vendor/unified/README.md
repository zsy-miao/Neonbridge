# victory-unified

A unified fork by **[Itsbatt](https://github.com/Itsbatt)** (built and tuned with **Antigravity / Google Deepmind AI**) combining the latest performance, shader/pipeline fixes, and rendering advances from **[fxceless/victory-fork](https://github.com/fxceless/victory-fork)**, **[Belobug/VIctory-Belobug-AMD](https://github.com/Belobug/VIctory-Belobug-AMD)**, **[DexNom/VIctory](https://github.com/DexNom/VIctory)**, and **[Zettrox/ztrx](https://github.com/Zettrox/ztrx)** on top of **[Marvisak/VIctory](https://github.com/Marvisak/VIctory)**.

**This repository contains no game files, code or assets.** You need your own
copy of the game. It is not affiliated with or endorsed by the game's
developer or publisher.

## What this unified fork adds

- **Graphics & Lighting:**
  - **Indirect diffuse / ambient light (`crates/victory-gpu/src/ambient.rs`, `VICTORY_AMBIENT=1.25`):** Built on `Belobug/VIctory-Belobug-AMD` (`4f45ab8`) and tuned with a higher `1.25` diffuse floor plus a subtle hemispherical sky-to-ground tint (`±8%`) so daylight outdoor shadows (foliage, terrain, buildings, character clothes) retain readable color and depth without overexposing interiors.
  - **Helper-lane quad reads (`crates/victory-shader/src/emit/lower.rs`):** Ported from `Belobug/VIctory-Belobug-AMD` (`03629f3`) and extended with `DexNom/VIctory` (`921d8d3`) fast quad intrinsics (`quadReadLaneAt`/`quadOp`) and quad-local `DPP8` support. Pixel shader quad operations preserve helper pixels so triangle edges no longer show lines on car paint, glass, and skin.
  - **Shader, cubemap & PSO fixes ported from `DexNom/VIctory` (`ddb7332`, `76749ce`, `5f4c3df`, `921d8d3`):**
    - Uniform branching across immediate static samplers (`lower.rs`, +40 shader variants).
    - Restored `wave64` scalar-copy lane mask recognition (`structure.rs`, voxel visibility).
    - Vertex output signature ordering by name offset, declared PSV interpolation, and matching unused packed PS inputs (`container.rs`, `emit/mod.rs`, `shader.rs`, fixing DXIL validation on 39 vertex shaders including 18 water shaders).
    - Cube-map explicit-gradient LOD fix (`LOG2_E` double-scaling bug removed + min-LOD clamp) and `CalculateLOD` spatial coordinate fix (`lower.rs`).
    - Integer render-target blend bypass on mixed MRTs (`pipeline/host.rs`, `pipeline.rs`, fixing strict D3D12 PSO validation) and depth-only pipeline pre-warming priority (`pipeline/make.rs`).
  - **Full `fxceless/victory-fork` rendering base (`0b0074e`):** Depth bounds test (`#82`), character bake/skin/hair texture rechecking (`VICTORY_HEAP_RECHECK`), first-room descriptor mirroring, `VICTORY_LEVELS_ON_DEMAND=1`, and 59/64 trace programs.
- **Performance, Stability & Input:**
  - **Hoisted indirect draw clamps (`indirect.rs`, `draw.rs`, `host.rs`, `list.rs`, opt-in `VICTORY_HOIST_CLAMPS=7`):** Ported from `Belobug/VIctory-Belobug-AMD` (`ccab973`, `afca189`, `918ab03`, `76387c2`). Moves indirect-draw argument clamping into a preceding command list with sequential barriers (`mode 7`). Defaults to `0` (inline clamps) on NVIDIA `late` queues to avoid per-frame list-split stalls while remaining available via `VICTORY_HOIST_CLAMPS=7`.
  - **Texture streaming & memory-range locking (`watch.rs`, `xmem.rs`, `texture.rs`, `descriptor/host.rs`):** Ported from `DexNom/VIctory` (`5f4c3df`, `921d8d3`). Replaces the global `MAPPING` lock with per-range locks (`title_memory_reading_range` / `title_memory_remapping_range`) + `join_placeholders` so DirectStorage remaps do not block unrelated texture uploads, keeps `O(1)` unmapped fast-path rejection in `mapped_bytes` (avoiding kernel `VirtualQuery` contention unless `VICTORY_COMMITTED_QUERY=1`), alternates retries with fresh textures (`TextureQueue::due`), and avoids dropping full remap queues (`record_remap` / `merge_remaps`).
  - **Tuned RTX 3070 8 GB launcher (`scripts/play.ps1`):** Uses `VICTORY_DISPLAY_MODE=1920x1080`, `VICTORY_ASYNC_SUBMIT=1`, `VICTORY_QUEUE2=1` (`VICTORY_QUEUE2_LANDING_MB=128`), `VICTORY_REFRESH_ONCE=1`, `VICTORY_COPY_PLAIN=1`, `VICTORY_CARD_BUFFERS_MIN_MB=1`, `VICTORY_CARD_BUFFERS_MB=768`, `VICTORY_MERGE_GAP=64`, `VICTORY_MERGE_RUNS=4096`, `VICTORY_JOIN_COPIES=1`, `VICTORY_CARD_REHASH_RUNS=1`, `VICTORY_VIEW_KNOWN=1`, `VICTORY_UNWATCHED_FRAMES=4`, `VICTORY_DISK_CORES=slow`, `VICTORY_PIPELINE_WAIT_MS=100`, `VICTORY_LATE_WAIT_LISTS=256`, `VICTORY_TEXTURE_BUDGET_MB=2560`, `VICTORY_TEXTURE_CATCHUP=0`, `VICTORY_TEXTURE_STAGE_MB=64`, and `VICTORY_TEXTURE_STAGE_MS=40`, while keeping `VICTORY_INDIRECT_WINDOW=0`, `VICTORY_HOIST_CLAMPS=0`, `VICTORY_LEVELS_ON_DEMAND`, `VICTORY_TEXTURE_NO_WAIT`, and `VICTORY_INPLACE` off for 8 GB NVIDIA stability.
  - **Safe descriptor lifetime (`crates/victory-gpu/src/descriptor/host.rs`, `descriptor.rs`, `host.rs`):** Defers live shader-visible descriptor `unmake` / view-remake writes whenever GPU submissions are in flight (`crate::host::flying()`), flushing them in `Host::sweep()` (`flush_idle_heaps()`) once the GPU is idle so executing shader warps never read torn descriptors.
  - **30 FPS frame cap (`VICTORY_MAX_FPS=30` in `crates/victory-gpu/src/queue.rs`):** Caps menus/loading screens cleanly at 30 FPS.
  - **Lower commit memory (`crates/victory-gpu/src/shadow.rs`):** Caps the idle snapshot pool at `64 MiB` and drains it on tight commit memory before retrying allocations.
  - **Keyboard + XInput pad support (`crates/victory-hle/src/modules/gameinput.rs`):** Maps `Space` alongside `Enter` to `A`, and keeps keyboard fallback buttons active on slot `0` while focused even when a physical or virtual XInput controller (e.g., DS4Windows) is connected.

## Quick Play

```powershell
cargo build --release -p victory
powershell -ExecutionPolicy Bypass -File scripts\play.ps1 -Game "<your game folder>\<the game's exe>"
```

## Tested Hardware & Status (2026-11-25)

Tested on **Microsoft Windows 11 Pro** (`10.0.26300`, 64-bit) using the **Xbox Series X** profile:

- **CPU:** AMD Ryzen 5 5600XT (6 cores / 12 threads)
- **GPU:** NVIDIA GeForce RTX 3070 (8 GB VRAM, driver `32.0.16.2114`)
- **Memory:** `31.93 GB` RAM, `47.93 GB` total virtual commit (`C:\pagefile.sys` set to `16384–24576 MB` on SATA SSD)

| Area | Verified Result on Test Machine |
|---|---|
| Boot, menus, start screen | Works; capped at `30 fps` (`VICTORY_MAX_FPS=30`); `Enter` / `Space` mapped to `A` |
| Progression reached | Advances past menus, opening intro, first room, and into gameplay (`Player` + `SARC0015` autosave reached; `ambient light` active) |
| Long-run stability | After fixing in-flight descriptor mutation (`768735a` / `flush_idle_heaps()`), ran **~58 minutes** continuously (`19:28:28Z` → `20:25:50Z`, zero crashes) into the outdoor camp mission (`8,011 draws/frame`, `3 fps` at `344.0 ms/frame`); earlier runs hit `nvlddmkm Event ID 13` `MMU Fault` (`0xf812b60` / `0x1104`, `0x887A0006`) after **49m 42s** and **49m 47s** (`18:49:15` → `19:39:02`) before that fix |
| Frame time & utilization | `425 ms` on a heavy `9,940`-draw frame under `diag` telemetry (`0.0 ms` CPU card wait, `2.2 ms` heap pass, `4.9 ms` texture refreshes, `77%` GPU busy); `~2+ fps` in normal `release` play |
| Memory footprint | `19.66–20.51 GB` commit / `15.13–15.40 GB` active RAM in-world (requires adequate Windows commit memory; `2–4 GB` pagefile caused OOM in `shadow::pooled`) |
| Known incompatibilities on this GPU | `VICTORY_ASYNC_PRESENT=1` crashed in `nvwgf2umx.dll` (kept off); `--console series-s` hit `0x887A0001` in `FFX_FSR4` (Series X used) |

## Credits

- **[Itsbatt](https://github.com/Itsbatt)** & **Antigravity (Google Deepmind AI)** — unification, performance tuning, memory pool hardening, frame limiter, and input fixes in this repository.
- **[fxceless/victory-fork](https://github.com/fxceless/victory-fork)** (`0b0074e`) — async submission/copy queue, plain-copy path, fast descriptor sync, depth bounds test (`#82`), trace shader translation, character texture recheck, and `VICTORY_LEVELS_ON_DEMAND`.
- **[Belobug/VIctory-Belobug-AMD](https://github.com/Belobug/VIctory-Belobug-AMD)** (`4f45ab8`, `03629f3`, `ccab973`, `afca189`, `918ab03`, `76387c2`) — indirect ambient light pass (`ambient.rs` / `VICTORY_AMBIENT`), pixel-shader helper-lane quad reads, and hoisted indirect-draw argument clamps (`VICTORY_HOIST_CLAMPS`).
- **[DexNom/VIctory](https://github.com/DexNom/VIctory)** (`ddb7332`, `76749ce`, `5f4c3df`, `921d8d3`) — uniform static-sampler branching, `wave64` scalar-copy mask recovery, vertex signature order & interpolation validation fixes, cubemap explicit-LOD & `CalculateLOD` fixes, quad-op/DPP8 improvements, integer-target blend bypass, and depth-only PSO pre-warming priority.
- **[Zettrox/ztrx](https://github.com/Zettrox/ztrx)** — streamlined Windows play script structure and defaults.
- **[VIctory](https://github.com/Marvisak/VIctory)** by Marvisak and its contributors is the base of everything here: the loader, the runtime services, the Direct3D 12 back end and the shader recompiler. Licensed GPL-3.0-or-later; so is this fork.
- **[XCL](https://github.com/ivall/xcl)** by ivall and its contributors (GPL-2.0-only) was used as a reference for how the same title behaves under another layer.
- **myokex** ([victory-amd](https://github.com/myokex/victory-amd), commit `c6dc4d8`, adapting PR #115 by Spexivus, joroyt, and #1122 by tightfall) and **Joshua Odom** (`10ea38e`) — memory page guards and read-beside fixes.
- Upstream `fxceless` changes were developed with [Claude Code](https://claude.com/claude-code).

## License & Upstream History

Licensed under **GPL-3.0-or-later** (see [LICENSE](LICENSE)). Full upstream changelog and testing history from [fxceless/victory-fork](https://github.com/fxceless/victory-fork) is preserved in [README.upstream.md](README.upstream.md).
