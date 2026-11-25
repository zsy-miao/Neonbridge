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
- Identity-heap header for xshader's e5ae009 clamp: unpaged heaps' last slot is a real descriptor, not null (836d91f).
- Pending A/B runs: `TUXBOX_DEPTH_TWINS=1`; `TUXBOX_RENDER_HEAP_EVICT` (now on); `TUXBOX_WAIT_HOIST` vs ad8e641; `TUXBOX_REFRESH_BUSY_SCALE` (the base env sets it to 1, which disables the x3 catch-up of 92823b8).

## Runs
- Turn the untested commits back on, one per run: 04a7bee, 81a0a3a, 65e77ca (stale shader serving).
- `TUXBOX_PIPELINE_SHAPE=fxceless` (449d9f7) against the bb35fd2 fps hold, then `TUXBOX_PIPELINE_HANDOFF_MS=0`.

## d3d12
- fxceless parity: items 7 and 2, then the c96b4f2 depth-bounds check.
- SetSampleMaskX failures in test145.
- Video gaps 4-6 and 9-10.
- Device memory went over budget in test145 (7913 of 6994 MiB) and free commit fell to 356 MiB.
- VIctory cd94d35 and 940d32f view_marks page marks: the d3d12 bitmap contract first, then the xshader half.

## xshader
- Five programs fail DXC with "^". Name them from the next prewarm's failure log (9a70469).
- Partition classifier (3d8ad77) records tb_partition; nothing translates by it yet.
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
- The master runs the ref watch.
- Open items to port (verdicts: side-session-notes refwatch-verify-verdicts.md):
  - xshader: TuxBox 1397cb6 with X2W a351c0fb/b8889849/a90c0fb: allow pixel_wave DPP16 quad exchange in loops (hlsl.rs:6286), gated on proven whole-quad EXEC (s_wqm_b64 exec,exec).
  - xshader: xcl 74fe4c2: v_div_fixup_f16 (opcodes.rs:991; tb_div_fixup + tb_f16_rne near hlsl.rs:9573).
  - xshader: X2W 6f784a50: v_dot2_u32_u16 and v_dot4_u32_u8 (next to v_dot4_i32_i8, hlsl.rs:8835).
  - xshader: xcl 1f8d4be: POPS ROV (operand 239 / hwreg 25 -> RasterizerOrdered PS UAVs).
  - runtime (optional): xcl 79fde18: add compile time to the compute_failure log (device/compute.rs:660).

## Done / not needed
- ported or not needed (reasons in refwatch-verify-verdicts.md): xcl c7f6933 fb9d059 6c9f947 18423c3 4070ae1 4b0af47 79fde18-partial; VIctory-zhai f07016a 0f11320 7f26331; five-star evict-windows 4fdfaed ff3914e c915760; TuxBox 97b8ad3; X2W 84c66897 (address views read the same memory) da888e46/efe54732/323ccf6f 28337fa6/9a6fd7ff 6d8ab3d3 9d013a48; leonida aa23d709c 22f2a04af/22171d092 545e8d51f f2268abea/255f17001; ScarlettCompat 446b178; ScarlettPort 7581c87; rebox 5d01c25; VItoriaBr c0b60a0.
- not needed: 0460750, audit found it unneeded.
- not needed: dd0f8b2, TuxBox calls its asserts symptom hiding.
