# Ray intersection

`image_bvh_intersect_ray` and `image_bvh64_intersect_ray` test one ray against one node of an AMD-format BVH (RT IP
1.1). Traversal stays in the program; the translator emits a call to `bvh_intersect_ray` in
`rdna_bvh.hlsli`, which follows AMD's own software model of the instruction (GPURT `IntersectCommon.hlsl`) operation
by operation. The game keeps AMD-format BVH memory, so only the instruction needs emulating.

## Operands

`vdata` receives 4 dwords. `vaddr` is a VGPR range or an NSA list:

| Dword (32-bit form) | Dword (bvh64) | Content |
|---|---|---|
| 0 | 0..1 | node pointer |
| 1 | 2 | ray extent (f32) |
| 2..4 | 3..5 | origin xyz (f32) |
| 5..7 | 6..8 | direction xyz (f32) |
| 8..10 | 9..11 | inverse direction xyz (f32) |

With `a16`, origin and extent stay f32 and the two directions are three packed f16 pairs: `dir.x | dir.y << 16`,
`dir.z | inv.x << 16`, `inv.y | inv.z << 16` (8 or 9 dwords). The instruction uses the given inverse direction for
boxes and the direction for triangles; it computes neither.

## T# (BVH descriptor)

| Field | Bits | Meaning |
|---|---|---|
| base address | dword0, dword1[7:0] | address bits 47..8; byte address = dword0 << 8 (bits above 31 dropped: 32-bit memory window) |
| box grow | dword1[30:23] | ULPs (of 2^-24) added to each box interval |
| box sort | dword1[31] | sort hit children by entry distance |
| size − 1 | dword2, dword3[9:0] | not checked |
| triangle return mode | dword3[24] | 1: t_num, t_denom, I, J; 0: t_num, t_denom, triangle_id, hit |
| type | dword3[31:28] | 8 = BVH |

The T# is a real address, not a heap index; nodes are read with `MemLoad`.

## Nodes

Node pointer bits [2:0] give the type; node address = base + ((pointer & ~7) << 3), 64-byte units.

| Type | Node | Layout |
|---|---|---|
| 0..3 | triangle (64 B) | V0 @0, V1 @12, V2 @24, V3 @36, V4 @48, triangle_id @60; vertices per type: 0 (V0,V1,V2), 1 (V1,V3,V2), 2 (V2,V3,V4), 3 (V2,V4,V0) |
| 4 | box16 (64 B) | 4 child pointers @0; per child 3 dwords @16 + 12i: min.x\|min.y, min.z\|max.x, max.y\|max.z (f16) |
| 5 | box32 (128 B) | 4 child pointers @0; per child min.xyz, max.xyz @16 + 24i (f32) |
| 6, 7 | instance, user | result 0xffffffff × 4 |

## Results

**Box nodes.** Per child: `t0 = (min − org) × inv`, `t1 = (max − org) × inv`; per axis entry/exit chosen by
`inv >= 0`; entry = max of the three, exit = min (IEEE maxNum/minNum: a NaN operand is ignored); entry clamped to
≥ 0, exit to ≤ extent; a NaN interval misses. The child hits when entry ≤ exit × (1 + grow × 2^-24). Missed children
return 0xffffffff. With box sort the four results are ordered by entry distance with the network (0,2)(1,3)(0,1)(2,3)(1,2),
misses last and ties in node order; without it they stay in node order.

**Triangle nodes.** `e1 = v1 − v0`, `e2 = v2 − v0`, `e3 = org − v0`, `s1 = cross(dir, e2)`, `s2 = cross(e3, e1)`;
`t_num = dot(e2, s2)`, `t_denom = dot(s1, e1)`, `I = dot(e3, s1)`, `J = dot(dir, s2)`. The ray misses when
`I/d < 0`, `I/d > 1`, `J/d < 0`, `I/d + J/d > 1` or `t_num/d < 0`; a miss returns `t_num = +inf`, `t_denom = 1`. The
extent is not tested; the sign of `t_denom` is the facing. Return mode 1 gives `{t_num, t_denom, I', J'}`, where I' and
J' are picked from `{t_denom − I − J, I, J}` by the triangle_id byte of the node type (bits 1..0 for I', 3..2 for J').

## Without a built structure

Without `--raytracing` a pipeline whose programs trace rays stays a stand-in (`RayTracingPrograms`): never built nor
loaded from the pipeline cache, its draws and dispatches skipped and logged once, as `--skip-pipelines` does. A
program is known to trace rays when it is translated (its HLSL includes `rdna_bvh.hlsli`), and in later boots or from
the compile host by a marker beside its DXIL in the program cache, `<name>.rt`. Boot w65b, with the node misses below
but the passes running, still lost the device about 100 s after the world loaded; with these pipelines skipped boots
ran 300 to 600 s. VIctory holds such pipelines until its builder has run (its PR #2311; facts only). Programs of `--translated-dir` have no marker and are not held back.


The loader builds no acceleration structure without `--raytracing` (XboxBvhBuilder builds nothing), so programs it
makes on demand define `FIVESTAR_BVH_MISS` (`BvhMiss`, part of the program cache identity): every node misses and
nothing is read. A box node returns four invalid children, a triangle node t_num inf over t_denom 1 (return mode 0:
hit_status 0). The game's traversal loops then end at once. Before, about 70 inline ray-tracing compute programs
(`CS_RaytracingResolve`, the ray-traced shadows, `bvh_*`) traversed whatever title memory the T# base pointed at,
following garbage child pointers until the driver reset the context: the world-load DEVICE_REMOVED of boot w63,
gone when those programs were skipped. VIctory found the same (its PR #2311: the world ray-tracing pass walked unbuilt
memory and hung NVIDIA at world entry) and holds such pipelines until its builder has run. Their loops are bounded besides by the loop guard (65,536 iterations per
compute invocation, [shader-translation.md](shader-translation.md#loop-guard)), which alone is no time bound for
such loops.

## Exactness

Every float operation is one IEEE f32 round-to-nearest operation in GPURT's order, written `precise` so the compiler
cannot contract or reorder it. The divisions of the hit test are done in f64 and rounded once (the correctly rounded
f32 quotient), so the program needs `DoublePrecisionFloatShaderOps`. EXEC applies: inactive lanes do not write
`vdata`.

## Unknown

- The internal precision of the hardware datapath; results may differ in the last bits and in hit decisions for rays
  exactly through edges or box corners.
- Denormal handling (PC GPUs may flush f32 denormals).
- Triangle node types 2 and 3 and return mode 0 (layout inferred).
- A triangle_id source index of 3 (returns 0.0 here).
