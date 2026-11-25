# TODO

Open work. Everything is paused until the port audit (bad ports, upstream reverts, places where the ref's method beats ours, unneeded ports) reports. Each item is tested in a run of its own.

## Runs
- test146: HEAD with only bb35fd2 (pipeline workers past two hold while the title presents under 30 fps). Off: `TUXBOX_TITLE_MEMORY_MB=0 TUXBOX_READ_COMMIT_CRT=0 NEONBRIDGE_STALE_SHADERS=0 TUXBOX_REFRESH_BUSY_SCALE=1`. Pass: intro near test134's 37.8 fps with clean audio.
- test147, only if intro videos 2 and 3 still garble: same build plus `TUXBOX_BINK_CATCHUP=0`.
- Then turn the untested commits back on, one per run: 04a7bee, 81a0a3a, 65e77ca (stale shader serving), 92823b8 (refresh budget for 1024+ textures).
- Test 753013e (page-mapping check) with `TUXBOX_ROOT_COMMIT_PAGE=1`. test145 logged CopyPageMappingsBatchX and unmap/split failures.

## d3d12 (held, unpushed)
- Gap 6: NOP the second watchdog site (test145 crashed at game+0x23cf683, a write to 0x0). ScarlettPort's VIctory does this.
- The 753013e page-mapping fix.
- Alias a GPU copy from R32_UINT to R9G9B9E5.
- Root SRV/UAV tables.
- Twin depth/stencil resources.
- Clamp indirect arguments into scratch.
- Heaps with n+1 null slots.
- Leonida slot-31 CPU-view mirror (CopyPageMappingsBatchX failures).
- SetSampleMaskX errors (new in test145).
- Ref port batches the research atlas has queued.
- Device memory went over budget in test145 (7913/6994 MiB) and free commit fell to 356 MiB.

## xshader (held, unpushed)
- Fence-only barriers (saved as fence_only.patch in xshader's scratch).
- Port the wave64 partition classifier.
- Descriptor-as-data gap.
- Find the 5 programs the prewarm fails with "^": DXC rejects their translated HLSL. The launcher logs only the 2 DXC timeouts by hash (35af8e6d, 40a6b3dc).
- Refs the atlas has queued.

## Launcher
- Log every failed shader by hash. Today only the summary line keeps the reason.

## Research
- The ref watch is paused (cron acf2e111 removed). Restart it after the audit.

## Housekeeping
- Delete `_ref/emu-old` once the audit no longer needs it.
