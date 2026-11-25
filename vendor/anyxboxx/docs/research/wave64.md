> Français : [wave64.md](../fr/recherche/wave64.md)

# Wave64 programs on 32-lane GPUs (issue #70)

## Why

Measured (2026-11-24, see [victory-xcl-fivestar.md](victory-xcl-fivestar.md)): the D3D12 debug layer refuses
119 of the title's compute shaders because they ask for 64-lane waves while the NVIDIA driver's maximum is 32.
RDNA 2 runs a compute program either in wave32 or in wave64 mode (`CS_W32_EN` in the dispatch state); a wave64
program is written for 64 lanes and cannot be translated as if it had 32.

## Wave64 state

The RDNA 2 ISA guide (chapter 3, "Kernel State") keeps the same registers and widens the lane masks:

| State | wave32 | wave64 |
| --- | --- | --- |
| Lanes | 0-31 | 0-63 |
| EXEC | `exec_lo` = s126 | `exec_lo` = s126 (lanes 0-31), `exec_hi` = s127 (lanes 32-63) |
| VCC | `vcc_lo` = s106 | `vcc_lo` = s106, `vcc_hi` = s107 |
| Lane mask in an SGPR | one SGPR | an aligned pair `s[n:n+1]` |
| VGPR | 32 values | 64 values |

Instruction families whose result depends on the wave size:

- **Compares** (VOPC, VOP3 compares, SDWA compares): the mask written to VCC, EXEC (`v_cmpx`) or an SGPR is
  64 bits wide; inactive lanes give 0.
- **Carries** (`v_add_co_u32`, `v_add_co_ci_u32`… VOP2 and VOP3b, `v_mad_u64_u32`): the carry in and the carry
  out are 64-bit masks.
- **`v_cndmask_b32`**: the selector (VCC, or SGPR pair in VOP3) is 64 bits.
- **Scalar mask operations**: the compiler uses the `_b64` forms (`s_and_saveexec_b64`, `s_andn2_b64 exec`,
  `s_or_b64 exec`, `s_mov_b64`, `s_bcnt1_i32_b64`, `s_ff1_i32_b64`…). Their semantics do not depend on the wave
  size; only the registers they read do.
- **Branches**: `s_cbranch_execz`/`vccz` test the whole 64-bit EXEC/VCC.
- **Lane access**: `v_readlane_b32`/`v_writelane_b32` take the lane number from bits [5:0] of SSRC1 instead
  of [4:0]; `v_readfirstlane_b32` takes the lowest active lane among 64 (lane 0 if EXEC is zero, as in wave32).
- **Lane ids**: `v_mbcnt_lo_u32_b32` counts the bits of S0 below the lane in mask bits [31:0] (all 32 bits for
  lanes 32-63), `v_mbcnt_hi_u32_b32` counts the bits of S0 below the lane in mask bits [63:32] (none for lanes
  0-31). The usual `v_mbcnt_hi(-1, v_mbcnt_lo(-1, 0))` gives the lane number 0-63.
- **Cross-lane data**: DPP rows stay 16 lanes (row_mask has 4 rows in wave64), DPP8 stays 8 lanes.
  `ds_bpermute_b32` only permutes **within each 32-lane half** on GFX10 wave64: Mesa's ACO compiler emulates a
  full-wave bpermute for "GFX10-11.5 wave64 mode" (`aco_select_nir_intrinsics.cpp`, read statically; not measured
  on hardware). The interpreter follows that: source lane = own half + `((address + offset) / 4) & 31`.

## Reference interpreter

`rdna2_exec.Wave(lanes=64)`; 32 stays the default and every existing program runs unchanged. The wave keeps
its lane count, `wave.exec` and `wave.vcc` read 64-bit masks (pairs), and every mask write goes through
`write_mask`, which writes one SGPR in wave32 and a pair in wave64. Only the instructions above change.

## HLSL strategy

Three ways to run a 64-lane program where the hardware wave has 32 lanes:

1. **`[WaveSize(64)]` with SM 6.6 wave intrinsics.** Refused by D3D12 on NVIDIA (the measured failure).
2. **Two halves on two 32-lane hardware waves** (xcl's approach, described in their `docs/wave64-on-wave32.md`,
   read as a design only: GPL-2.0-only; VIctory PR #189 mode `Wave32`). Fast, but it relies on threads
   `32k..32k+31` of a group landing in one hardware wave in lane order, which D3D12 does not guarantee (PR #117
   found NVIDIA packing pixels of two triangles in one warp; measured: WARP runs a 64-thread group as 16 waves of
   4 lanes), so every cross-lane
   operation would need a runtime `WaveGetLaneCount()` check and a fallback anyway.
3. **One thread group of 64 threads is the virtual wave** (VIctory PR #189 mode `Emulate`, GPL-3.0-or-later).
   Lane `n` is the thread with `SV_GroupIndex = n`; lane masks are built in `groupshared` memory with
   `InterlockedOr` between group barriers, and lane reads go through a 64-entry `groupshared` array.

**Chosen: 3.** It is exactly what our wave32 translation already does for 32 threads (`Ballot`, `ReadLane`),
so it relies only on what D3D12 guarantees (`GroupMemoryBarrierWithGroupSync`, `SV_GroupIndex`), runs on any
hardware wave size (NVIDIA 32, AMD 32/64, WARP), needs no `WaveSize` attribute and no shader model above 6.0,
and can be checked byte for byte against the interpreter on both the RTX and WARP. Barriers stay correct because
RDNA 2 control flow is scalar: every thread holds an identical copy of the SGPRs, SCC, EXEC and VCC, so every
branch is uniform across the group and every thread reaches every barrier.

Trade-off: each compare, carry and lane read costs three group barriers instead of one wave instruction. That
is the same cost as our wave32 path today; option 2 can be added later as a faster path once a measured shader
needs it, keeping 3 as the reference it is checked against.

HLSL layout in wave64: `[numthreads(64, 1, 1)]`, `s[127]` = `exec_hi` initialised to `0xFFFFFFFF`, a
`LANE_ON` macro (`Bit64(s[126], s[127], lane)`) instead of `(s[126] >> lane) & 1`, `Ballot64` returning the two
mask words, `ReadLane64` with `& 63`. The 64-bit scalar instructions (`s_mov_b64`, logic, `saveexec`, `wrexec`,
`s_bcnt*_b64`, `s_ff*_b64`, `s_cselect_b64`, `s_cmp_*_u64`) are added for both wave sizes.

## Supported in wave64 and refused

Translated in wave64 and checked byte for byte against the interpreter (`tests/test_rdna2_hlsl_wave64.py`):
64-bit scalar mask operations (`s_and_saveexec_b64`, `s_andn2_b64`, `s_or_b64`, `s_mov_b64`, `s_bcnt1_i32_b64`,
`s_ff1_i32_b64`, `s_xor_b64`, `s_cmp_lg_u64`), `s_cbranch_execz`/`vccnz` loops and if/else, compares into VCC, EXEC
(`v_cmpx`) or an SGPR pair, a ballot whose mask spans both halves read back as data, VOP2 and VOP3 carries,
`v_cndmask_b32` (VOP2 and VOP3), `v_mad_u64_u32`, `v_readlane_b32`, `v_readfirstlane_b32`, `v_writelane_b32`,
`v_mbcnt_lo/hi_u32_b32`, `ds_bpermute_b32` (half-wave), LDS and global memory. Measured: identical on an RTX 4060
Laptop GPU, whose `WaveGetLaneCount()` is 32 (threads 32-63 of the group are lanes 0-31 of a second warp), and with
WARP (4-lane waves). The remaining 64-bit scalar forms (`saveexec`/`wrexec` variants, `s_not_b64`, `s_nor_b64`,
`s_bcnt0`, `s_ff0`, `s_cmov_b64`, `s_cmp_eq_u64`) are checked the same way in a wave32 program.

Delegated unchanged to the wave32 translation, with `LANE_ON` as the activity test: every other lane-wise ALU,
64-bit and memory instruction (buffer, scalar and image loads included); they do not depend on the wave size.

Refused with `Untranslatable` naming the instruction: DPP16/DPP8 (cross-lane, not yet written for 64 lanes),
SDWA compares and SDWA `v_cndmask_b32` (32-bit masks), `v_mad_i64_i32` with a carry (already refused in wave32),
and any instruction whose wave32 translation reads EXEC or VCC directly or does a lane exchange that is not in
the list above: the wave64 translator checks the text it delegates to the wave32 code and refuses anything that
still reads `s[106]`/`s[126]` bit by bit or uses `Ballot`, `ReadLane` or `lane` (reading VCC or EXEC as a value,
such as `v_mov_b32 v1, vcc_lo`, stays allowed).

Not covered: the wave size is not read from the dispatch state yet; callers pass `wave_size=64`.
