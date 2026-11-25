# TODO

Open work. Everything is paused until the port audit (bad ports, upstream reverts, places where the ref's method beats ours, unneeded ports) reports. Each item is tested in a run of its own.

## Runs
- test146: HEAD with only bb35fd2 (pipeline workers past two hold while the title presents under 30 fps). Off: `TUXBOX_TITLE_MEMORY_MB=0 TUXBOX_READ_COMMIT_CRT=0 NEONBRIDGE_STALE_SHADERS=0 TUXBOX_REFRESH_BUSY_SCALE=1`. Pass: intro near test134's 37.8 fps with clean audio.
- test147, only if intro videos 2 and 3 still garble: same build plus `TUXBOX_BINK_CATCHUP=0`.
- Then turn the untested commits back on, one per run: 04a7bee, 81a0a3a, 65e77ca (stale shader serving), 92823b8 (refresh budget for 1024+ textures).
- Test 753013e (page-mapping check) with `TUXBOX_ROOT_COMMIT_PAGE=1`. test145 logged CopyPageMappingsBatchX and unmap/split failures.

## d3d12 (nothing started, no patches; all of d3d12's work is pushed)
- Gap 6 in the memory atlas: NOP the second watchdog site, game+0x23cf683, where test145 crashed on a write to 0x0. VIctory hooks.rs:25-45 does this.
- Find why presents stopped after #1959. Title threads were waiting at game+0x221c3c2 and game+0x4da2892.
- test145 logged page-mapping, split-unmap and SetSampleMaskX failures. 753013e is the suspect; `TUXBOX_ROOT_COMMIT_PAGE=1` reverts it.
- Intro videos 2 and 3 garble. The suspect is the catch-up replay in xmem bink.rs; test it with `TUXBOX_BINK_CATCHUP=0` first, before any code.
- fxceless parity, in this order: item 3 (an alias GPU copy from R32_UINT to R9G9B9E5), then items 1 (root SRV/UAV tables), 4 (twin depth/stencil resources), 6 (clamp indirect arguments into scratch), 7 and 2, then the c96b4f2 depth-bounds check.
- Heaps with n+1 null slots.
- Leonida slot-31 CPU-view mirror (the CopyPageMappingsBatchX failures).
- Research refs: TuxBox 3f2a717, VIctory cd94d35 and 940d32f, leonida f2268abea and 255f17001.
- Older queue: memory-atlas gaps 4-10, video gaps 3-6 and 9-10, pipeline gap 5, and the earlier ref batches (xcl, five-star, X2W, rebox, ScarlettCompat, leonida A8).
- Device memory went over budget in test145 (7913 of 6994 MiB) and free commit fell to 356 MiB.

## xshader (tree clean at 1befb53, nothing pushed)
- Fence-only barriers inside per-thread guards (fxceless lower.rs 1391-1419). The patch compiles and sits in xshader's scratch as fence_only.patch.
- Five programs fail DXC with "^". These are DXC rejections, not panics. Name them by logging every failed shader by hash (see Launcher).
- 35af8e6d and 40a6b3dc hit the DXC timeout. xshader translates each in 7 s, but each emits 12.7 MB of flat HLSL: a ~3,300-case v_cmpx_ne v31,k chain. Fix by emitting it as a switch, or by raising the DXC timeout or lowering its -O level for huge programs.
- Partition classifier (xcl dfc5578, 5796cc4, 43a5550): written in xshader's scratch (partition.rs, partition_cfg.rs), not wired in yet.
- xcl 7bea019 (split-wave64 mask liveness per block) and 9e2c7a5 (fold the reduction-tail readlane).
- Atlas gaps 1-9:
  - descriptors read as data (at least refuse them)
  - mask-as-data ballot
  - runtime V# stride
  - safe barriers
  - wave64 split for any group size
  - a retry ladder
  - irreducible-CFG split
  - leftover cross-lane refusals
  - votes in resolve_wave32, wave_cull and packed
- VIctory cd94d35 view_marks, shader half. It waits on d3d12's bitmap binding.
- BVH intersection (7 refusals). Needs console BVH nodes from the runtime.

## Launcher
- Log every failed shader by hash: remove the `if m.contains("timed out")` gate at src/shaders.rs:1112, so every Failed prints `Xbox shader {key}: {m}`. The next prewarm then names all 7.

## Research
- The ref watch is paused (cron acf2e111 removed). Restart it after the audit.

## Housekeeping
- Delete `_ref/emu-old` once the audit no longer needs it.
