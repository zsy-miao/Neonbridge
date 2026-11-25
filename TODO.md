# TODO

Open work. Each item is tested in a run of its own.

## In flight
- xmem page join fails with Err(87) (71ac658 logs the failing OS step).
- Unmap/split failures: the join87 unmap:256 errors wait on test157's a06f6c7 region lines. CopyPageMappingsBatchX itself is not needed: test156 mapped every batch with no refusals, and leonida 255f17001's CPU-view mirror serves its d3d12 reading GPU-only memory at a paired CPU view (title_gpu_views::to_cpu), which ours never does (we import at the title address), so a mirror would only add aliased views.
- Presents stop at #1959 / #1600, at game+0x23cf735. Needs a d3d12_xs pdb.
- Pending A/B runs: `TUXBOX_DEPTH_TWINS=1` (close-time pass landed, 86f630f late_pass); `TUXBOX_ROOT_VIEW_TABLES=1` (bind half landed, 92da1f3; default-on after its run); `TUXBOX_RENDER_HEAP_EVICT` (now on); `TUXBOX_REFRESH_BUSY_SCALE` (the base env sets it to 1, which disables the x3 catch-up of 92823b8); `TUXBOX_PRESENT_RELEASE_ALL=1` (five-star b828157); `TUXBOX_SAMPLER_DECODE=fxceless` (check slot 61 lines).

## Runs
- A/B for the untested commits, one per run:
  - 04a7bee: never tested on, because the base env sets `TUXBOX_TITLE_MEMORY_MB=0`. B: drop it from the env, giving the 13824 MiB cap. Compare commit growth and the working set budget line. Safe.
  - 81a0a3a: never tested on, because the base env sets `TUXBOX_READ_COMMIT_CRT=0`. B: drop it from the env. Watch for short CRT reads (998) in the video atlas. Safe.
- A/B `TB_VIEW_MARKS=1` (498fc37, 9549a0d; needs a prewarm): shadow write-backs should copy only marked pages.
- `TUXBOX_PIPELINE_SHAPE=fxceless` (449d9f7) against the bb35fd2 fps hold, then `TUXBOX_PIPELINE_HANDOFF_MS=0`.

## d3d12
- Exact shadow write-back per command list (parked): commands.rs records each list's descriptor-table ranges, root UAVs and copy/clear targets; shadow.rs writes back only the shadows whose UAV handles fall in them. 5c45bf2 made 55ecc4b's window opt-in (TUXBOX_SHADOW_RECENT=N).
- Device memory went over budget in test145 (7913 of 6994 MiB) and free commit fell to 356 MiB.

## xshader
- xcl 7bea019 (split-wave64 mask liveness per block).
- Gaps:
  - descriptors read as data (at least refuse them)
  - mask-as-data ballot
  - runtime V# stride: loads done (heap raw views read tb_slot_views +12, VItoriaBr b62bbc0; sibling stride TuxBox 7b815f2). Left: IDXEN stores (refused, as TuxBox 63343eb) and root-table raw views with no slot (no ref reads a stride there). Fallback design: give root raw views a runtime stride word beside the t5+ space32766 AS views (Coder5), then lift the store refusal behind a TB_ switch.
  - safe barriers beyond guarded S_BARRIER
  - wave64 split for any group size: done (396b1f0, xcl several-console-waves form, TB_SPLIT_WAVE64_MULTI=0 off).
  - a retry ladder
  - irreducible-CFG split: interleaved loops already fall back to the tb_pc dispatcher (hlsl.rs:6556); the only refusal left is a compute DPP quad exchange with interleaved loops (hlsl.rs:6559). Ref: xcl gpu/dxil/cfg.rs make_reducible (splits the innermost multi-entry region, capped at 4x blocks). Port plan: when that refusal hits, check true irreducibility; if irreducible, duplicate the region's instructions past the program end with fresh offsets and retarget the extra entries (as exit_stubs moves stubs), then re-run the structured form. Behind TB_CFG_SPLIT; port only once a cache mark shows the refusal.
  - leftover cross-lane refusals
  - votes in resolve_wave32, wave_cull and packed: no cache mark carries their refusals (unresolved wave/scan ballot, conflicting wave widths); these are our own recognizers with no ref counterpart. A program they refuse still has the split wave64 form (now any group size). Revisit only if a mark appears.
- BVH intersection (7 refusals). Needs console BVH nodes from the runtime.

## Research
- Partition classifier (3d8ad77) records tb_partition; no ref translates by it (xcl declares the module, never calls it). Parked until one does.
- xcl b4019c4 + 7bea019: split-wave64 mask liveness (masks read only lane by lane stay in their half; 7bea019 is that solver's per-block speed-up). Ours exchanges every mask (split_wave64.rs:12). A port of x64_masks.rs (1.2k lines, x64-recompiler IR) to hlsl is worth it only if exchange cost shows in a wave64 profile.
- The master runs the ref watch.
- Open items to port (verdicts: side-session-notes refwatch-verify-verdicts.md):
  - ref watch (storage) to port:
    - xshader (Coder4): VItoriaBr 67bb579 (select between two immediate samplers by a static-sampler range); rebox 18c049c (a buffer V# below 4 GiB not in GPU memory reads as empty: a float base word faulted the GPU); five-star 97dafaa (64-bit image atomics as uint64 typed atomics).
    - d3d12 (Coder5): leonida d16eb1ff0 (a depth placement inside another one's extent gets its own image), 89fdb83e0 (a console DS texture with UAV is created without it); ScarlettCompat 93c0f96 (map the console layout word before CreatePlacedResourceX).
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

## Done / not needed
- not needed (ref watch, d3d12): leonida 028a414fa (DEVICE11_QUEUE_SLOTS already puts queue_fence at 16/17, commands.rs:3365); leonida adc4f314b (RTV/DSV heaps are native here, only CBV_SRV_UAV gets arenas; UAV clears already translate the CPU handle, depth.rs:64; ClearDepthStencilView is a typed fn, depth kept); ScarlettCompat dd0e557 (layout 0x102 = 256 + swizzle 2, the 256 B display microtile, xg-layout compute_microtile; the 4 KiB chain is 261 and already stores smallest first with one shared tail tile; its 8-level tail cap never binds at 4 KiB); VItoriaBr faca57e (placement/upload.rs refresh already checks its time limit before each texture, caps bytes mid-texture and resumes from a rotating cursor).
- not needed (ref watch): five-star 22f2085/79a5a3e d16 format buffer halves (hlsl.rs:11595 already packs two per VGPR); five-star 95c3cd9 format element range rule (our buffers are D3D views, bounds checked by the view, no V# range emulation); five-star 1a8083a superstep carry (off by default there, no superstep dispatch here); ScarlettCompat 8b8511f/a88e509 BCPack (6ixpack's bcpack tool owns decoding, not the runtime); palmetto 755e947..772fa54 (Vulkan frame-capture replay and slot-70 docs; no D3D12 path change).
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
