# BVH builder on the GPU (design)

The title's indirect light, reflections and lamp and sun shadows are ray-traced in its own compute programs: a
per-thread stack, loads through raw title addresses, and `image_bvh*_intersect_ray` for the node test, which we
emulate ([ray-intersection.md](ray-intersection.md)). The structures they walk come from `XboxBvhBuilder.dll` (XDXR,
[raytracing.md](raytracing.md)). Our shim replaces that DLL: by default it builds nothing, and with `--raytracing` it
builds on the CPU at record time. This note is the plan for running the DLL's own GPU builder instead. Status: design,
plus an offline first step; nothing runs it yet.

## What the game expects

| Item | Finding |
|---|---|
| Constructor | `XboxBvhBuilder(device, false)` makes the GPU builder: a compute pipeline per builder program through device slot 62 (its first one fails on the desktop, so our shim never runs the constructor) |
| Programs | 52 DXBC containers in the DLL, each with an XDAT native program; `fivestar shader embedded XboxBvhBuilder.dll <out>` writes them as `.dxbc` and `.fssm` |
| Builds | `BuildRaytracingAccelerationStructureBatched2X` records the builder's dispatches onto the game's command list; the GPU runs them when the list runs, so a BLAS of skinned geometry (`bvh_skinning~CS...` writes its vertices on the GPU) is built from vertices that exist only then |
| Rehydration | `RehydrateRaytracingAccelerationStructureBatched2X` on the GPU, or the CPU export we already use |
| Memory | the programs find their buffers by raw title address in a spilled user-data table and root views; loads, stores, atomic add/sub and float min/max reach title memory by address (our address map and page views) |
| Static world | pre-built serialized BLASes in `core_bvh4.rpf`, rehydrated in place |

The DLL's strings name its stages: `LoadTriangles`, `LoadInstances`, `CalculateMortonCodes(64)`, the bitonic and
radix sorts (`Bitonic{Pre,Inner,Outer}Sort(64)CS`, `RadixSortPass`), `PrefixSum64` to `PrefixSum4096`,
`BuildNodeGraphLBVH`, `BuildBVHSplits(64)`, `FindTreelets`, `TreeletOptimize`, `ConstructBVH4`, `RefitBVH4`,
`ComputeInstanceBounds`, `SplitInstances`, `RearrangeBVHs`, `RearrangeTriangles`, `CopyAndCompactBVH`,
`CopyForBVHUpdate`, `CopyCounters`, `Box16Rehydration`, `Box32Rehydration` and `LeafRehydration`: an LBVH build
over Morton codes, treelet optimisation, a BVH4 pass, refits and compaction.

## Offline inventory (2026-11-25, translate and compile for wave32)

42 of the 52 programs translated and compiled at first; 10 refused on `buffer_load_ushort` (3), `global_atomic_fmax`
(3), `global_atomic_sub` (3) and `global_atomic_fmin` (1), and one more then on `buffer_store_short`. With those (byte
and short buffer loads and stores, atomic sub, and float min and max ordered as integers, WARP cases
`cs_buffer_subdword`, `cs_buffer_subdword_store` and `cs_atomic_sub_fminmax`) all 52 translate and compile.
VIctory's fork reached 49 of 52 after adding the same kinds of access (facts only, THIRD_PARTY_NOTICES.md).

## How we would run it

1. Translate all 52 (done offline), and check the builder's stages against the ISA model on WARP.
2. Run the real constructor with our device: slot 62 makes each program through the usual program path, with its
   pipeline a stand-in until the driver is done.
3. Bind the builder's entry points to the DLL again (with `--raytracing-gpu`), so it records its dispatches on the
   game's lists through our command-list slots.
4. Gate: the builder's dispatches run only in a frame where every one of its pipelines is built; otherwise all are
   skipped and the structures keep what they hold (the CPU path may fill them meanwhile).
5. A quarantine mode first: the builder's stores by address go to a scratch copy, not title memory, and a probe
   compares its output with the CPU builder's.

## Risks

- A partly built builder wrote garbage bounds into title memory and crashed the title (VIctory's fork); hence the
  all-or-nothing gate and the quarantine run.
- Stores and atomics by raw address must land where the title's own programs read: the address map's page views
  cover title memory, but a page without a view loses the access (a read gives zero, a store is dropped).
- Wave64 programs on 32-lane GPUs: compute programs with lane exchanges take the superstep or group-wave forms; their
  cost is unmeasured.
- The CPU path and the GPU path writing the same structure must not both run: one owner per structure.
- Driver compile time for 52 more pipelines at the first build.

## Inventory (`--raytracing-gpu`)

The shipped constructor runs on the game's object with our device (`BvhBuilderInventory`): every compute pipeline it
asks for through slot 62 while it runs is logged (its CS container's offset in the module and size, its root
signature) and answered with a stand-in that is never built, so it completes and no builder program runs. The
builder's entry points stay ours (the CPU paths with `--raytracing`, nothing without). A boot with it says which
device slots the constructor uses besides 62 and whether it completes on our device; a native exception in it would
end the process, which is why it is a separate flag.

## First step

`fivestar shader embedded <module> <out>` carves a module's bare containers and writes their native programs, so the
builder can be translated and tested offline (`fivestar shader translate <out>/fssm ...`). It changes nothing at run
time.
