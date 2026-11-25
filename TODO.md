# TODO

Open work. Each item is tested in a run of its own.

## In flight
- Intro runs at 14 fps: shadow copies of about 191 MB each way per submit, under lazy commit. test149 runs with `TUXBOX_LAZY_COMMIT=0 TUXBOX_IMPORT_REMAPPABLE=1`.
- xmem page join fails with Err(87) (71ac658 logs the failing OS step).
- CopyPageMappingsBatchX and unmap/split failures (838c433, 3df971b narrowed the imports; leonida slot-31 CPU-view mirror is the ref model).
- Presents stop at #1959 / #1600, at game+0x23cf735. Needs a d3d12_xs pdb.
- Root view tables (37fe84c): the bind half is missing; `TUXBOX_ROOT_VIEW_TABLES=1` stays opt-in until it lands.
- Depth twins (00b029c, 000f78a, 86f630f): the close-time pass.
- VItoriaBr b62bbc0 (descriptor addresses by computed heap index, SLOT_VIEWS): waits on a runtime per-slot view-address table.
- Pending A/B runs: `TUXBOX_DEPTH_TWINS=1`; `TUXBOX_RENDER_HEAP_EVICT` (now on); `TUXBOX_WAIT_HOIST` vs ad8e641; `TUXBOX_REFRESH_BUSY_SCALE` (the base env sets it to 1, which disables the x3 catch-up of 92823b8); `TUXBOX_PRESENT_RELEASE_ALL=1` (five-star b828157); `TUXBOX_SAMPLER_DECODE=fxceless` (check slot 61 lines).

## Runs
- A/B for the untested commits, one per run:
  - 04a7bee: never tested on, because the base env sets `TUXBOX_TITLE_MEMORY_MB=0`. B: drop it from the env, giving the 13824 MiB cap. Compare commit growth and the working set budget line. Safe.
  - 81a0a3a: never tested on, because the base env sets `TUXBOX_READ_COMMIT_CRT=0`. B: drop it from the env. Watch for short CRT reads (998) in the video atlas. Safe.
  - 65e77ca: stale shader serving is off in test runs, because the launcher sets `NEONBRIDGE_STALE_SHADERS=0` (main.rs:198,546). A/B: `NEONBRIDGE_STALE_SHADERS=1` on a run right after an xshader landing. Risk: it mixes xshader builds (see risk 65e77ca), so it voids that run's shader results.
- A/B `TB_VIEW_MARKS=1` (498fc37, 9549a0d; needs a prewarm): shadow write-backs should copy only marked pages.
- `TUXBOX_PIPELINE_SHAPE=fxceless` (449d9f7) against the bb35fd2 fps hold, then `TUXBOX_PIPELINE_HANDOFF_MS=0`.

## d3d12
- Exact shadow write-back per command list (parked): commands.rs records each list's descriptor-table ranges, root UAVs and copy/clear targets; shadow.rs writes back only the shadows whose UAV handles fall in them. 5c45bf2 made 55ecc4b's window opt-in (TUXBOX_SHADOW_RECENT=N).
- fxceless alias remainder (gaps file item 2): an R9G9B9E5 view over a UAV-only R32_UINT texture that compute writes. 5e3a1ee covers only render-target owners, and UAV-only images cannot be castable (allocation.rs:37). Port VIctory-fxceless resource.rs:1187-1237: per-subresource GPU copies through a buffer when the alias is read after the owner was written.
- Device memory went over budget in test145 (7913 of 6994 MiB) and free commit fell to 356 MiB.
- VIctory cd94d35 and 940d32f view_marks page marks: the d3d12 bitmap contract first, then the xshader half.

## xshader
- Five programs fail DXC with "^". Name them from the next prewarm's failure log (9a70469).
- xcl 7bea019 (split-wave64 mask liveness per block).
- Gaps:
  - descriptors read as data (at least refuse them)
  - mask-as-data ballot
  - runtime V# stride
  - safe barriers beyond guarded S_BARRIER
  - wave64 split for any group size
  - a retry ladder
  - irreducible-CFG split
  - leftover cross-lane refusals
  - votes in resolve_wave32, wave_cull and packed
- BVH intersection (7 refusals). Needs console BVH nodes from the runtime.

## Research
- Partition classifier (3d8ad77) records tb_partition; no ref translates by it (xcl declares the module, never calls it). Parked until one does.
- xcl b4019c4 + 7bea019: split-wave64 mask liveness (masks read only lane by lane stay in their half; 7bea019 is that solver's per-block speed-up). Ours exchanges every mask (split_wave64.rs:12). A port of x64_masks.rs (1.2k lines, x64-recompiler IR) to hlsl is worth it only if exchange cost shows in a wave64 profile.
- The master runs the ref watch.
- Open items to port (verdicts: side-session-notes refwatch-verify-verdicts.md):
  - xshader: TuxBox 1397cb6 with X2W a351c0fb/b8889849/a90c0fb: allow pixel_wave DPP16 quad exchange in loops (hlsl.rs:6286), gated on proven whole-quad EXEC (s_wqm_b64 exec,exec).
  - xshader: xcl 1f8d4be: POPS ROV (operand 239 / hwreg 25 -> RasterizerOrdered PS UAVs).
  - runtime (optional): xcl 79fde18: add compile time to the compute_failure log (device/compute.rs:660).

## Housekeeping
- Delete `_ref/emu-old` once the audit no longer needs it.

## Port audit findings (each fix gets a run of its own)

### d3d12
- ref better: 689199f (a placeholder pipeline drops draws; blend 19/20 done).
- f40b107: the table arena never frees (VIctory HostHeap release + c1c3281 scan fix are better).
- five-star b828157 (e1ce01d): release every pending memory wait at each present (79dff41 releases only waits no queued list writes).
- five-star 226887c (f96284e): trigger on the slot-74 64-bit wait.
- ab5af3f: snapshot every CBV no title buffer covers.
- 2e53f61: reopen a closed list with a native Reset.
- Symptom fix 00f5205: the placeholder pipeline hides refusals. Chase them through the refused() reasons.
- Sampler decode (views.rs:1346): the mag/min order, aniso rule, compare range and the w[12] bias differ from fxceless sampler.rs:11-48. Verify against live words.
- Small fixes:
  - confirm GTA VI never calls Scarlett slot 59;
  - 88dbc97/d97535c: parity-only unless their notices fire.

### xshader
- five-star b9b57e2 (with the 8c0372e idxen guard).
- upstream: TuxBox e2860b5/4a518ef (literal samplers), VIctory ffe310e/75677cb.
- dead/unneeded: src/dxil, 2d36d48 saturate, 692983f, dbd0bcd tb_rndne.
- minor: 3258c66 denorm, 83f5a3b unaligned, da066d0 clamp/omod, e7f6be4, 078b76a cube, credit fixes.

### runtime/launcher
- Risk 65e77ca: stale shaders mix xshader builds. Keep NEONBRIDGE_STALE_SHADERS=0 for test runs.
- b69a488: re-evaluate f1b5622 now that the AnyPS5 ban is lifted.

## Done / not needed
- xshader: xcl 74fe4c2 v_div_fixup_f16 ported (9250d13, TB_DIV_FIXUP_F16=0 off).
- xshader: X2W 6f784a50 v_dot2_u32_u16 / v_dot4_u32_u8 ported (70d0b20, TB_DOT_UNSIGNED=0 off).
- not needed: leonida 273b3bdf9 (c63e099): neither the exe nor any 6m DLL imports ReadFileEx/ReadFileScatter; ReadFile already commits before the read (read_commit.rs:77).
- not needed: 28e3f08 keep the window's protection (leonida 8d4dd7a8/80ed5dd3: protect & ~(WRITECOMBINE|NOCACHE|GUARD), NOACCESS -> READWRITE): every window xmem accepts (virtual_memory.rs:438, 0x801/0x04/0x2004/0x2404/0x10404/0x22004) reduces to PAGE_READWRITE, which views already get.
- ported or not needed (reasons in refwatch-verify-verdicts.md): xcl c7f6933 fb9d059 6c9f947 18423c3 4070ae1 4b0af47 79fde18-partial; VIctory-zhai f07016a 0f11320 7f26331; five-star evict-windows 4fdfaed ff3914e c915760; TuxBox 97b8ad3; X2W 84c66897 (address views read the same memory) da888e46/efe54732/323ccf6f 28337fa6/9a6fd7ff 6d8ab3d3 9d013a48; leonida aa23d709c 22f2a04af/22171d092 545e8d51f f2268abea/255f17001; ScarlettCompat 446b178; ScarlettPort 7581c87; rebox 5d01c25; VItoriaBr c0b60a0.
- not needed: VItoriaBr a91ef70: no cursor-wrapping dynamic descriptor scan here (descriptor_paging assigns slots in order).
- not needed: VItoriaBr 32d7848: query resolve already bounds offset+count*stride with checked_add (compute::buffer_offset); its heap-lock/mirror parts have no counterpart.
- not needed: VIctory-zhai 9f24a38: XMemAllocDefault uses HeapAlloc/pools/VirtualAlloc, never an over-aligned Rust Layout, so nothing is padded twice.
- not needed: 0460750, audit found it unneeded.
- not needed: dd0f8b2, TuxBox calls its asserts symptom hiding.
