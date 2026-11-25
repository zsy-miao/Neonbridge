# TODO

Open work. Each item is tested in a run of its own.

## In flight
- Pending A/B runs: `TUXBOX_RENDER_HEAP_EVICT` (now on); `TUXBOX_REFRESH_BUSY_SCALE` (the base env sets it to 1, which disables the x3 catch-up of 92823b8); `TUXBOX_SAMPLER_DECODE=fxceless` (check slot 61 lines).

## Runs
- `TUXBOX_PIPELINE_HANDOFF_MS=0` (fxceless shape default on since 717accd; =0 restores the fps hold).

## d3d12
- Memory: system commit is the danger, not device memory. test179: private 7.5 -> 23 GiB, free commit down to 267 MiB, the jumps at heavy driver compiles that kept their arena on the compiling thread (+2.1-2.9 GiB each). Device local over budget (8788 of 7188 MiB: page pools 4720, images 3330, shadows 1070, render heaps 848) is paged by WDDM: idle pools are demoted, render heaps evicted, placed textures trimmed and unmapped. Re-measure free commit in test181 with 5793004 (fresh thread per heavy compile); then try `TUXBOX_PAGES_SYSTEM_PCT` (new sampled-texture pools to system memory over budget; off since test59 died at 1 GiB of commit).

## xshader
- xcl 7bea019 (split-wave64 mask liveness per block).
- Gaps:
  - descriptors read as data (at least refuse them)
  - mask-as-data ballot
  - runtime V# stride: done for heap raw views (tb_slot_views +12, VItoriaBr b62bbc0; sibling stride TuxBox 7b815f2) and compute root raw views (tb_root_raw_stride t4 space32766, 288a936 + d3d12 9841a78), loads and stores (b1155aa, TB_RAW_IDXEN_STORE=0 refuses). Left: graphics-stage root raw views (the runtime binds tb_root_raw_stride for compute only); port only once a cache mark shows the refusal.
  - safe barriers beyond guarded S_BARRIER
  - wave64 split for any group size: done (396b1f0, xcl several-console-waves form, TB_SPLIT_WAVE64_MULTI=0 off).
  - a retry ladder
  - irreducible-CFG split: interleaved loops already fall back to the tb_pc dispatcher (hlsl.rs:6556); the only refusal left is a compute DPP quad exchange with interleaved loops (hlsl.rs:6559). Ref: xcl gpu/dxil/cfg.rs make_reducible (splits the innermost multi-entry region, capped at 4x blocks). Port plan: when that refusal hits, check true irreducibility; if irreducible, duplicate the region's instructions past the program end with fresh offsets and retarget the extra entries (as exit_stubs moves stubs), then re-run the structured form. Behind TB_CFG_SPLIT; port only once a cache mark shows the refusal.
  - leftover cross-lane refusals
  - votes in resolve_wave32, wave_cull and packed: no cache mark carries their refusals (unresolved wave/scan ballot, conflicting wave widths); these are our own recognizers with no ref counterpart. A program they refuse still has the split wave64 form (now any group size). Revisit only if a mark appears.
- BVH intersection (7 refusals). Needs console BVH nodes from the runtime.
  - Stand-in: TB_BVH_FLAT_ZERO reads zeros for getpc-less flat loads in BVH programs (no hit). Replace with TB_MEM_DIR page-directory reads (tb_mem_load) once the runtime half lands, then port the node tests (MESIA intersect.rs / bvh.rs).

## Research
- Partition classifier (3d8ad77) records tb_partition; no ref translates by it (xcl declares the module, never calls it). Parked until one does.
- xcl b4019c4 + 7bea019: split-wave64 mask liveness (masks read only lane by lane stay in their half; 7bea019 is that solver's per-block speed-up). Ours exchanges every mask (split_wave64.rs:12). A port of x64_masks.rs (1.2k lines, x64-recompiler IR) to hlsl is worth it only if exchange cost shows in a wave64 profile.
- The master runs the ref watch.
- Open items to port (verdicts: side-session-notes refwatch-verify-verdicts.md):
  - ref watch (storage) to port:

## Housekeeping
- Delete `_ref/emu-old` once the audit no longer needs it.

## Port audit findings (each fix gets a run of its own)

### d3d12
- Sampler decode: five-star ConsoleSamplerDescription (from the title's filler 0x1412b96b0) is default (TUXBOX_SAMPLER_FIVESTAR=0 back to fxceless): filters 1 point/2 linear/3 aniso, bias w[12], compare = any w[16] past NEVER. Confirm in the next run's slot 61 words that no filter word is 0.
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
- Presents stop at game+0x23cf735 (main thread on the frame thread's DONE semaphore, a210cf8) from the ring-credit wait (0x2a42d0 at 0x2a442d): gone. No hang or wait stack in test152-180 holds 0x2a442d, and the release at game+0x24825a7 shows as write-watch hits every run from test159 on. No single fixing commit found. The 0x23cf735 stops left (test173/175/177/179) have the frame thread at game+0x23cf4f2 in the Subrender group wait (game+0x43c3d5d); those are fixed by b01c21d (BIND_SKIP_HEAVY), 5793004 (TITLE_NO_JOIN, STORE_DEFER) and d2a259a (SAVE_SNAPSHOT).
- not needed (ref watch, d3d12): leonida 028a414fa (DEVICE11_QUEUE_SLOTS already puts queue_fence at 16/17, commands.rs:3365); leonida adc4f314b (RTV/DSV heaps are native here, only CBV_SRV_UAV gets arenas; UAV clears already translate the CPU handle, depth.rs:64; ClearDepthStencilView is a typed fn, depth kept); ScarlettCompat dd0e557 (layout 0x102 = 256 + swizzle 2, the 256 B display microtile, xg-layout compute_microtile; the 4 KiB chain is 261 and already stores smallest first with one shared tail tile; its 8-level tail cap never binds at 4 KiB); VItoriaBr faca57e (placement/upload.rs refresh already checks its time limit before each texture, caps bytes mid-texture and resumes from a rotating cursor).
- not needed (ref watch): five-star 22f2085/79a5a3e d16 format buffer halves (hlsl.rs:11595 already packs two per VGPR); five-star 95c3cd9 format element range rule (our buffers are D3D views, bounds checked by the view, no V# range emulation); five-star 1a8083a superstep carry (off by default there, no superstep dispatch here); ScarlettCompat 8b8511f/a88e509 BCPack (6ixpack's bcpack tool owns decoding, not the runtime); palmetto 755e947..772fa54 (Vulkan frame-capture replay and slot-70 docs; no D3D12 path change). Xbox2Windows 19e04be9 (threads faulting on a site another thread rewrote: console_cpu.rs emulates every hit in a vectored handler and rewrites no code, so the race cannot occur). palmetto 6416a27 (pairs slot-70 texture publications with recent creations because palmetto refuses slot 70: ours decodes it as views::address_srv with its resource description and texture placement lookup) and 18365e8 (Vulkan-backend readback instrumentation; our test runs already screenshot the window every 5 s).
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
