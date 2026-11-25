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
- Fence-only barriers inside per-thread guards (fxceless lower.rs 1391-1419). The patch compiles: `_ref/held/fence_only.patch`.
- Five programs fail DXC with "^". These are DXC rejections, not panics. Name them by logging every failed shader by hash (see Launcher).
- 35af8e6d and 40a6b3dc hit the DXC timeout. xshader translates each in 7 s, but each emits 12.7 MB of flat HLSL: a ~3,300-case v_cmpx_ne v31,k chain. Fix by emitting it as a switch, or by raising the DXC timeout or lowering its -O level for huge programs.
- Partition classifier (xcl dfc5578, 5796cc4, 43a5550): written but not wired in yet: `_ref/held/partition.rs` and `_ref/held/partition_cfg.rs`.
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
- The ref watch is stopped (cron acf2e111 deleted). Restart it after the audit.
- Open atlas items to port:
  - TuxBox 1397cb6: pixel_wave quad exchange in loops (hlsl.rs:5855).
  - TuxBox 5891be3 and 4b40c64: immediates in v_add/sub_nc_i32/_i16.
  - TuxBox 9ba53f9: mask HOST_FLAGS (heap.rs:320, 526, 669, 690).
  - TuxBox 3f2a717: a stand-in for the large-compile deadline, plus a failure ledger.
  - VIctory cd94d35 and 940d32f: view_marks page marks. The d3d12 contract comes first, then xshader.
  - leonida f2268abea and 255f17001: the slot-31 paired CPU view mirror.
  - xcl 7881dbc: BC7 dictionary entry 0 (xcrdapi bcpack_victory/tables.rs:1443).
  - xcl 74fe4c2: v_div_fixup_f16.
  - xcl 1f8d4be: POPS ROV.
  - X2W 6f784a50: v_dot2_u32_u16 and v_dot4_u32_u8.
- To verify:
  - xcl c7f6933: a zero T# that falls back to heap[0] hangs.
  - xcl fb9d059: the 32 MiB texture upload cap.
  - xcl 6c9f947: fmt_encode clamps for UINT/SINT.
  - xcl 18423c3: round-to-nearest-even on f16 products.
  - xcl 4070ae1: 2D resinfo for D16.
  - xcl 4b0af47: a per-stage mesh_stage cache.
  - VIctory-zhai f07016a: copy-memory diagnostics.
  - five-star: evict-windows (design only).

## Housekeeping
- Delete `_ref/emu-old` once the audit no longer needs it.

## Port audit findings (fix before new ports; each fix gets a run of its own)

### d3d12 slice 2
- BAD PORT 7214656/112d2fd: the alias places below the guest VA and retires only fully covered spans (xbox-emulator alias.rs:76/103).
- upstream: rebox fe7f1d4/7745cef (don't retire mip/slice views or textures still in use), xcl 971c0e5 (3D packed mip tails off pages), X2W a6879558 (D24S8 + MSAA plane sizes), five-star bd100ba (release held waits at 250 ms).
- ref better: b16f2f9 (clip write-watch spans), 689199f (alpha blend factor 19/20; a placeholder pipeline drops draws).
- a0f014d TUXBOX_BACK_IMAGES is a symptom workaround and should default off. 0460750 isn't needed.

### d3d12 slice 3
- BAD PORT f2cc6d8: layout 0 is always mapped to 0x105, so RT and depth miss 64KB_R_X/Z_X (VIctory resource.rs:150-157).
- 4852a1e/dbe0bbc: xcl b55c478 isn't ported, so released lists keep their allocators referenced.
- rebox a1ebac2: the present-time release of unsignalled waits is missing.
- dd0f8b2: TuxBox calls its asserts symptom hiding, so skip them.
- f40b107: the table arena never frees (VIctory HostHeap release + c1c3281 scan fix are better).
- c6c3416: the merge 61c1359 regressed TB_DFDEBUG=0 at 4 sites.

### xs slice 0
- BAD PORT b9bcd25: one shared never-reset loop guard; the cap doesn't kill the lane. Use a per-loop counter (fxceless lower.rs:10906), a default of 4096+, and find the real cause.
- BAD PORT f112928: parked mask kept stale. Port fxceless ir.rs:309-380.
- ref better 57ff63d: wave64 halves barrier-only. Use xcl 066ae5b's bounded spin.
- ref better 1c3b05c: mask-vs-number registers. Port five-star b5fe3a7/48c08b8.
- upstream: TuxBox 0b6e788 (pixel_heap_base rooted), 5891be3 (int32 immediates), 63343eb (IDXEN strict is opt-in here).
- dead/unneeded: 8b14d05 clamp + --target-writes-out, 5377d53 arm, src/dxil, 2d36d48 saturate, 692983f.
- minor: 3258c66 denorm, 83f5a3b unaligned, 4799d57 VM start, da066d0 clamp/omod, e7f6be4, 078b76a cube, credit fixes.

### xs slice 1
- BAD PORT 8c0372e: the idxen guard ignores OOB_SELECT, so select 1/3 drop valid accesses (five-star buf_limit). Also port five-star b9b57e2.
- misread d51e710: compare samples silently read mip 0. Default to refuse (fxceless lower.rs:9430, X2W d21e3d6e).
- misread f8d4963: s_memtime reads 0, but the cited ref refuses it. Default to refuse.
- upstream: VIctory 645391b (exact unorm/snorm ends), TuxBox e2860b5/4a518ef (literal samplers), VIctory ffe310e/75677cb.
- ref better: a1df9a6 (refuse SOPP 0x26).
- risk: 2ad360a (SNORM _pck).
- unneeded: dbd0bcd tb_rndne, 14b2de5 TB_FLAT_UINT.

### d3d12 slice 1
- BAD PORT 49702da: the lazy commit resumes on any committed page and never checks the access type, so a write to a read-only page or a DEP fault re-faults forever. Port xcl lazy.rs `allows`.
- BAD PORT db43389: misread leonida 673dd15d. D3D12 doesn't need the depth clamp, so default `TUXBOX_DEPTH_CLEAR_CLAMP` off and fix the credit.
- Partial port 479b039/631b28f: port five-star 9d61a3c (skip unready first uses) and xcl 9a272d4 (a pipeline the title waits on skips the queue).
- Upstream changed since we copied:
  - rebox c78d9f4/f0bcabe (2be3430): build display-sized pipelines at once, at the front of the queue.
  - five-star b828157 (e1ce01d): release every pending memory wait at each present.
  - five-star ad07dd9 (0217153): give up on a timed-out address for good, with a 5000 ms timeout.
  - five-star CompileBudget limits (65344bb): a 4-compile cap, a 28 GB ceiling, start one when none runs (9a5bcc1), urgent compiles.
  - TuxBox 9ba53f9 (68bb418): mask flags with HOST_FLAGS at heap.rs:669/690.
  - TuxBox ff2cdfb (0173b3f): re-import with unordered access allowed before the shadow copy, and record memory_type at +56.
  - five-star 226887c (f96284e): trigger on the slot-74 64-bit wait.
- Ref method better:
  - 7404809: re-check every 50 ms whether the title now waits on a prefetch.
  - 68d8158: chunk copies over 64 MiB through scratch (xcl 392dc0c).
  - ab5af3f: snapshot every CBV no title buffer covers.
  - 2e53f61: reopen a closed list with a native Reset.
- Symptom fix 00f5205: the placeholder pipeline hides refusals. Chase them through the refused() reasons.
- Gap cefdfdb: set stencil when part 0 is added, and handle or refuse tags 26/27/28.
- Small fixes:
  - move the misplaced doc comment in src/main.rs;
  - correct the device.rs:627 doc;
  - map B8G8R8X8_TYPELESS for present;
  - check GTA VI's imports before keeping the XError exports;
  - confirm GTA VI never calls Scarlett slot 59.

### d3d12 slice 0
- BAD PORT 4e749f4: we write TRIANGLE where VIctory passes the guest topology, or UNDEFINED (stream.rs:723, :516).
- Bug 4b28545: the indirect clamp is gated at bind time, so an indirect call whose pipeline is ready by call time runs unclamped. Decide at call time (commands.rs:1656).
- Ref method better:
  - 7d7963e: move the wait to the segment start instead of skipping it, and lift the 32-split cap.
  - 8807bc8: make STORES an RwLock and call StorePipeline outside LIBRARY.
  - c5018ee: apply the cap of 4 and the 28 GB ceiling to every compile, and port five-star 4e95ab2 (leave out write-combined memory).
  - 92823b8: our refresh base is 32 MB / 4 ms against fxceless's 192 MB / 250 ms; likely the cause of the backlog.
  - bb35fd2/ac966c7: the fps hold treats the symptom. Test the fxceless shape (n/2 workers, prefetchers off), then TUXBOX_PIPELINE_HANDOFF_MS=0.
  - Sampler decode (views.rs:1346): the mag/min order, aniso rule, compare range and the w[12] bias differ from fxceless sampler.rs:11-48. Verify against live words.
  - 184d486: port five-star AsRaw.
  - 77d9d04: link mesh+pixel before the create.
- Small fixes:
  - 9ccbb3e: clamp the rounded-up CBV to the buffer end.
  - 733f135: flush the commit_guard log before exit.
  - c4f8bd8: the credit should be VIctory 35b054e.
  - 88dbc97/d97535c: parity-only unless their notices fire.
