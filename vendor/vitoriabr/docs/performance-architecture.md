# Performance Architecture: Shader Translation, Pipeline Management, and CPU–GPU Synchronization

This review covers VItoriaBr main at [`e7f9af536ed32411d3af2cd9c2a634020d069ea4`][baseline] and public contributions as of November 25, 2026. The first investigation proposed here is a warm-cache reproduction of the AMD regression in [#18][issue18], alongside a separately controlled NVIDIA run. The question is where the current build spends time once translation and pipeline misses are accounted for. The existing shader warm-up and asynchronous paths provide useful control cases.

**Evidence.** Source-confirmed findings describe the pinned implementation. Repository-reported measurements are inherited comments, project reports, or contributor observations. Literature-supported findings refer to public hardware and API contracts. Pending contributions are open at the review date. Architectural hypotheses and research directions require measurements or additional correctness work. This review contributes no new gameplay results.

## Execution model

```text
Xbox offline    HLSL → compiler IR → RDNA 2 code generation → console GPU
Windows D3D12   HLSL → DXIL → host driver compilation → native GPU code
VItoriaBr GPU   console code → decode → SSA → recover control flow and bindings
                → emit DXIL → host D3D12 PSO → native AMD or NVIDIA GPU code
VItoriaBr CPU   mapped x86-64 title code → native CPU execution
                → console API bindings and host Windows services
```

**Literature-supported and source-confirmed.** Microsoft's public Xbox HLSL sample documents offline console shader compilation and an optional ACO graphics PSO serialization flow. The title's specific toolchain and runtime compilation behavior remain open. Conventional Windows D3D12 passes DXIL, an LLVM-derived intermediate representation, to a host driver. VItoriaBr must first recover resource bindings, control flow, and lane semantics from native console shaders. Even where the host also uses RDNA 2, the current path reconstructs the D3D12 inputs and uses the host driver. The loader independently maps the x86-64 title code and handles its imports. [Xbox HLSL sample][gdk] · [DXIL specification][dxil] · [VItoriaBr architecture][arch] · [Runtime translation][shader-objects]

## Submission and memory visibility

**Source-confirmed.** Title queues share a host direct queue and already split work around guest waits. Several source comments describe earlier defaults. With the switches unset in a normal run, the current code enables relay stage 2, selects late-wait stage 3 through relay, and enables wide back. Use `VICTORY_RELAY=0` with late wait unset as a synchronous control. That path batches lists under a lock through preparation, submission, fence waiting and copy-back. Capture effective settings and whether each path actually ran alongside each measurement. [Guest waits][queue-waits] · [Late wait and relay][late] · [Relay enablement][relay-default] · [Synchronous fallback][sync] · [Wide back][wide]

| Path | What it changes and what to check |
| --- | --- |
| Late wait and relay | Late wait stages defer completion, reduce unconditional settling, and at stage 3 allow separate present copies. Relay 1 moves ordered CPU copy-back onto its own thread. Stage 2 uses declared-range hazards to admit more work in flight. Check overlapping CPU reads and writes, queue fences, present, and bounded resource retirement. Per-list GPU timing disables late wait, and kept snapshots disable relay. [Late stages][late] · [Guest fence][fence] |
| `VICTORY_WIDE_BACK` | Parallel CPU snapshot copies and per-range comparison. Observe memory bandwidth and landing time separately from wide merging and GPU execution. [Shadow workers][wide] |
| `VICTORY_ASYNC_SUBMIT` and `VICTORY_ASYNC_PRESENT` | Opt-in ordered preparation and submission on a dedicated CPU worker, with optional present through that path. Submission requires late wait and wait cutting. Present has additional eligibility checks. Validate guest signal visibility, queued-present latency, captures, and resize before comparing frame pacing. [Submit worker][async] · [Present gating][present-async] |
| `VICTORY_QUEUE2` | Opt-in copy queues for readback fetches and uploads. The relevant bits can be measured separately. Cross-queue fences, bounded landing buffers, and fallback to the direct queue matter for interpreting transfer times. [Queue policy][copy-queue] · [Queue ordering][copy-order] |

**Repository-reported measurements.** The inherited run 86 comment reports 446 ms per frame, with 134 ms preparing, 194 ms waiting and 78 ms copying back. Those figures are from synchronous, instrumented per-list waits and account for 406 ms of the reported total. The waiting field measures CPU elapsed time around synchronization. Use GPU timestamps and queue occupancy to allocate costs in a current trace. The later sync from victory-fork adds frame/activity diagnostics, async paths, indirect draw windows, and direct CPU copy work. Its source-history correction withdraws the original 6.0 to 8.9 FPS draw-window comparison because the scenes differed. The revised comparison of 145 ms and 124 ms at similar draw counts used one RTX 4070 run per configuration and changed both windows and copy behavior. The effect of either option alone remains unknown. [Run 86][run86] · [Imported changes][import] · [Corrected comparison][corrected]

**Architectural hypothesis.** Reduce waits only around dependency edges that a trace shows to be unnecessary. Start by comparing existing relay and submit modes on the same scene. Keep unknown descriptor ranges, aliasing, CPU polling, and forced guest waits conservative. Increasing batch size or frames in flight requires fence-backed lifetimes for command allocators, descriptor slices, snapshots, and back buffers. Compare ready-to-submit latency, hardware queue gaps, CPU wakeups, and actual display intervals. Additional shader cache coverage could reduce first-use stalls while leaving these warm-frame constraints largely intact. [Multi-engine synchronization][multi-engine] · [Fence-based lifetimes][fence-lifetime]

**Literature-supported and source-confirmed.** Xbox Series X and S share physical GDDR6 between CPU and GPU, with published bandwidth regions. The discrete AMD and NVIDIA devices of interest have physically separate system memory and VRAM. Software translation cannot reproduce the console's physical memory topology. The existing shadow path mirrors selected allocations, records CPU writes where supported, and uploads or reads back ranges around dependencies. Large eligible shadows can already use GPU-local memory and 64 KiB changed-piece comparisons, subject to local memory budget. Multiple views and texture aliases can refer to the same title bytes, so per-view dirtiness alone is insufficient. [Xbox Series X][series-x] · [Xbox Series S][series-s] · [Shadow strategy][shadow] · [Aliased writes and on-demand work][alias-commit]

**Repository-reported measurement.** A former implementation repeatedly allocated and filled CPU snapshots of overlapping views of a 174 MB heap and was reported to incur about 2.3 million CPU page faults per world frame. Current code pools snapshot storage and avoids copying the same overlapping bytes repeatedly. Separate current CPU snapshot faults from GPU residency events. The README also reports an RTX 5060 Ti configuration in which fetching card buffers inside their flights reduced the copy-back field while increasing the field labeled as GPU time and worsening FPS. Treat those fields as measurements of that run's configuration and timing mode. [Snapshot history][snapshots] · [README tuning observations][readme-tuning] · [In-flight fetch][fetch]

**Research direction.** Compare declared, changed, and transferred bytes. Refine existing dirty information where a finer mask or coalescing policy reduces total work. [Issue #4][issue4] also points to upstream producer-side page marking. Identify GPU-only producers and consumers for longer GPU-local residency. Defer readback only to a verified CPU visibility point. Card shadows have a default 1 MiB size threshold and 1,024 MiB allocation cap, with texture twins counted separately. The README preset raises the cap to 6,144 MiB. Compare device-local and nonlocal budgets, texture reservations and private commit under both configurations. The existing in-place path pins eligible aligned ranges and forbids freeing, decommitting, or changing protections while a heap refers to them. Its wider level bypasses shadow write tracking. Test that level with late wait explicitly disabled. Evaluate CPU-write/GPU-read staging buffers separately with GPU Upload Heaps, gated on feature support and local budget. ReBAR still transfers CPU writes over PCIe and CPU reads from VRAM can be costly. [Card shadow admission][shadow] · [Texture residency][texture-budget] · [In-place restrictions][inplace] · [Readback visibility][readback] · [Video memory budgets][residency] · [GPU Upload Heaps][gpu-upload] · [NVIDIA upload heap analysis][nvidia-upload]

## Shader translation and Wave64 correctness

**Source-confirmed.** The translator reconstructs register-dword SSA and preserves old masked vector values, raises EXEC to lane predicates, and traces descriptor and scalar memory provenance to root parameters and heaps. Symbolic whole-wave analysis can recognize reductions and prefixes. Existing passes recover lane-local bit algorithms and thread uniform control flow. The emitter removes dead code, folds integer operations, reuses dominated values, propagates branch facts, and coalesces redundant same-block loads subject to effects. These are established compiler techniques applied after recovering semantics from a binary. [SSA][ir] · [Control flow][structure] · [Resource tracking][resource] · [Wave analysis][wave] · [Uniform threading][threading] · [Lane-local reasoning][own-bits] · [Cleanup passes][opt] · [LLVM passes][llvm]

**Source-confirmed and literature-supported.** An RDNA 2 Wave64 can span two NVIDIA 32-lane warps. Some operations can use independent halves after proof that they cannot communicate. Others require cross-half exchange using group shared memory and convergent synchronization. The emitter includes exact repairs, wave-wide retries for selected refusals, and approximate fallback paths. The current barrier mode defaults to 2. Explicit mode 1 retains group waits selectively for title barriers, and mode 0 removes group rendezvous even from translator-generated exchanges. An app-visible D3D12 group barrier must satisfy convergence. Preserving a memory fence alone cannot preserve a required rendezvous. The encoded loop cap is a correctness-sensitive fallback because reaching it exits guest work early. [RDNA 2 ISA][rdna] · [NVIDIA warp model][warp] · [Retry and fallback][wave-select] · [Barrier and loop settings][group-sync] · [Host barrier contract][group-barrier]

**Contribution status.** VItoriaBr incorporated triippywhale's descriptor sentinel work and already includes host wave and heap facts in the shader key. The current [ZTRX #20][ztrx20] and [VIctory #1133][victory1133] cover related descriptor-cap behavior. One difference worth validating is VItoriaBr's handling of null descriptor initialization failures after range registration. The automatic barrier proposal now lives in [ZTRX #30][ztrx30] and [victory-fork #608][fork608]. It limits fence-only lowering to classified divergent title barriers on known narrower hosts for single-console-wave groups. VItoriaBr has no automatic mode at this baseline. Cross-wave producer–consumer validation remains necessary. [Local sentinel history][clamp-commit] · [Local heap creation][descriptor] · [Null initialization][descriptor-null]

In the separate circular allocator discussion, [#8][pr8] and [#9][pr9] closed unmerged. A subsequent commit fixes the wraparound test fixture cited in [#14][issue14] and [#16][issue16]. Merged work includes Lilith133T's indexed-view stride fix [#7][pr7], mathcx's LDS bounds [#10][pr10] and texture sampling [#15][pr15], Yoshi-233's texture layout support [#11][pr11], and UnkwnDev's packed dot products [#12][pr12]. The team can use the reported diagnostic PSO and synthetic GPU/WARP results to choose regression cases. Runtime visuals and whole-title coverage require separate checks. [Fixture correction][fixture]

**Research direction.** Start with equivalence checks for divergence, uniform values, redundant EXEC work, and reconstructible collectives. Differential tests should exercise sparse and cross-half masks, helper lanes, loop exits, LDS/UAV visibility, precision, rounding and denormals. Use the existing synthetic and differential tests as the starting point. Once equivalence holds, measure register pressure, occupancy, spills and group shared-memory traffic. A lower loop limit, per-half substitution, or barrier removal can improve a timing number while changing rendering or persistent output. [Differential test harness][diff-tests] · [AMD occupancy guidance][occupancy]

## Pipeline preparation, scheduling and identity

**Source-confirmed.** Three workflows have different boundaries.

| Workflow | Current result |
| --- | --- |
| `victory warm` | Merged in andysilva86's [#6][pr6]. Discovers accessible containers, trims each to its declared extent, deduplicates by full hash, and writes translated shader records through the runtime cache. The cache writer initializes host capabilities. [Warm entry point][warm] · [Shared cache writer][shader-key] |
| `victory translate --warm <file\|all>` | The other warm-up route uses a hash list or all found containers, a time-limit option, and the runtime writer. This route passes the source's extracted bytes and deduplicates its diagnostic hash, so compare runtime hits and input extents before assuming that both warm-up routes cover identical entries. [Selective warm-up][translate-warm] |
| Plain `victory translate` with optional `--validate` and `--pipelines` | Translation census, optional DXIL validation and diagnostic PSO construction. Tests may use stand-in stages or selected pairs. The diagnostic counts describe this construction path. [Command routing][commands] · [Diagnostic pipeline checks][translate-checks] |

Runtime containers are shared by full content hash and memoize their translation. The D3D12 pipeline library separately keys host PSOs by pipeline kind, root signature, translated shader code and state. It retains a mapped blob, falls back to normal creation when necessary, and saves compatible driver artifacts. The bounded early list records requested title pipeline keys and starts matching work when the title recreates those objects. It lacks the full inputs needed for independent replay. [Runtime shader objects][shader-objects] · [Content cache][cache] · [Pipeline library][library] · [Early requests][early] · [D3D12 compatibility contract][d3d-create-library]

**Research direction.** Reuse existing warm-up and collect a complete, normalized local PSO manifest from representative runs. Record stage associations, root signatures and effective fixed state including formats, samples, input or mesh state and other relevant pipeline fields. Replay it through the actual host constructor, then serialize the compatible library. Keep a fallback for combinations that are generated, streamed, or observed only at run time. Accessible shader containers place a bound on shader coverage, while actual PSO coverage also depends on the combinations of code and state used. Epic's automatic and bundled PSO routes show complementary discovery and capture strategies. Valve Fossilize illustrates capture and replay of Vulkan creation descriptions on target hardware. The closed, unmerged [victory-fork #282][fork282] is prior work on populating both caches. [Host PSO creation][pso] · [Epic precaching][epic-precache] · [Epic bundled caches][epic-bundle] · [Fossilize][fossilize] · [Asset policy](asset-policy.md)

**Source-confirmed and architectural hypothesis.** The shared budget allows up to **500 ms per title frame** for pending host pipelines, about 30 times a 60 Hz frame budget. Workers run below normal priority and default to half the logical processors, clamped to 2–8. Requested traditional vertex/graphics work precedes the requested mesh/compute band and then early work, preserving FIFO within each band. Lowering the wait can skip already-requested draws and dispatches. A missed one-time compute initialization can corrupt later frames even after its PSO becomes ready. Measure queue age and starvation by band and distinguish critical producer passes from optional work. During offline warm-up, bound workers by commit and I/O pressure. During loading, prioritize upcoming dependencies. In menus and gameplay, measure contention with render and audio threads, pipeline-library calls and the host driver before changing admission or priority. [Budget and accounting][budget] · [Queue bands and workers][workers] · [Priority commit][priority]

**Source-confirmed and architectural hypothesis.** The shader cache already uses the translator's Rust source fingerprint, serializer source, build flags, discovered `VICTORY_*` option values and host wave/heap facts. `VICTORY_SHADER_CACHE_KEY=exe` adds the executable hash as an opt-in. The warm-up guide's executable-wide default describes earlier behavior. A finer identity might reduce misses caused by nonsemantic source edits or diagnostic settings, but a source-only fingerprint also warrants coverage checks for dependency or compiler inputs. Before narrowing it, test an explicit decoder/IR/emitter/serialization ABI contract and complete effective capability inputs. Rebuild or reject on incompatibility and independently test native library adapter/driver matching. [Warm-up guide](precompilacao-shaders.md) · [Cache key][shader-key] · [Source and switch discovery][shader-build] · [Translation fingerprint][fingerprint]

Each title object has its own pending job and memoized result. The pipeline library serializes same-name loads, then calls the driver outside that lock after a miss. Equivalent objects may therefore compile concurrently. Count duplicate in-flight misses before testing per-device job sharing keyed by a complete serialized host description. Keep unrelated requests concurrent and verify failure, retry, and lifetime semantics. [Pending job][pending] · [Library load and create][library-key]

## API options and ray-tracing correctness

| Technology | Applicability |
| --- | --- |
| D3D12 Pipeline Libraries | Already in use. Compare compatible-library loads, misses, saves, mapped memory, and driver rejection before changing the cache. [API contract][d3d-library] |
| Partial Graphics Programs | Agility SDK 1.620 retail since October 28, 2026. Reusable pre-raster and pixel portions can reduce repeated compilation. Microsoft's initial retail support matrix lists AMD Radeon RX 7000/9000 and lists NVIDIA as planned. Measure only with an appropriate driver and a passing capability query. [Retail release][partial-retail] · [Specification][partial] |
| DirectX Advanced Shader Delivery | Microsoft's October 21 announcement covers supported Windows 11 24H2+ hardware. The distribution workflow matches precompiled data to adapter family and compiler ABI. It identifies the Xbox PC app and Microsoft Store, with Steam and Epic rollout described as forthcoming at that date. VItoriaBr distribution eligibility and workload coverage are open. Cache misses still fall back to the local driver. [Availability][asd] · [Registration API][asd-api] · [Explicit PSDBs][psdb] |
| Vulkan pipeline extensions | Graphics pipeline libraries separate reusable parts. Shader objects use a different dynamic-state model. Pipeline binaries expose application-managed, compatibility-keyed data. All require device support. These are comparative designs. Keeping D3D12 primary preserves DXIL, root-signature, heap, PSO and synchronization semantics already present in this layer. [Graphics pipeline library][vk-gpl] · [Shader object][vk-so] · [Pipeline binary][vk-bin] |

**Source-confirmed and repository-reported.** The title traverses guest-format acceleration structures in compute and uses AMD instructions to accelerate individual node intersections. VItoriaBr includes software DXIL node tests and reference comparisons, with an opt-in builder gated on available pipelines. The builder defaults off. A separate opt-in holds tagged traversal dispatches until the builder gate reports all observed builder pipelines ready. That signal marks readiness to dispatch. Its README records incomplete builder coverage on the reported NVIDIA configuration and a failed shortcut that made more pipelines available while corrupting memory. A synthetic node comparison establishes agreement with the project's reference for those inputs. Collect evidence with the builder and traversal states recorded explicitly before attributing baseline cost to ray tracing. [Ray-tracing design](fork-ray-tracing.md) · [Software intersections][intersect] · [Builder and traversal gates][bvh-gate] · [Related VIctory work][victory2311] · [README RTGI observation][readme-tuning]

**Research direction.** DXR exposes driver-owned acceleration structures and traversal via TraceRay or RayQuery. It does not expose an AMD node fetch and test primitive that can be replaced directly with NVIDIA RT Cores. Investigate recovered geometry, instancing, update/refit behavior and ordering or precision only after the software path yields correct lighting and stable guest resources. [DXR specification][dxr]

## Measurement and correctness gates

**Repository-reported measurements.** Issue [#18][issue18] compares an RX 6500 XT custom-preset run on this baseline with older commit `18924c7`, reports multi-second frames and visible face-texture regressions despite a completed warm-up, and spans both a shader merge and a renderer sync. Its older result is documented in [#13][issue13]. The reports differ in storage context and leave the cause open. [#1][issue1] offers NVIDIA correctness cases and [#5][issue5] records an AMD driver failure on a separate configuration. Use repeated runs with matching scene, title data, save/input, resolution, adapter, driver, storage, settings and cache state. Collect a comparison on each vendor independently. Test the known-good and reported bad commits with the same preset first. If the regression reproduces, separate shader misses or refusals from texture upload delays, memory pressure and synchronization, then isolate changes. [Texture refresh fallback][texture-refresh]

| Controlled comparison | What it can establish |
| --- | --- |
| Empty shader cache and library versus warm shader cache and empty library | Translation overhead versus residual driver pipeline construction. Record and control the separate driver disk-cache state. |
| Warm shaders and compatible warm library | Sustained rendering costs, with remaining cache misses reported explicitly. |
| Known-good versus reported bad revision | Reproducibility of the scene-specific regression and a starting point for revision isolation. |
| Runtime defaults versus synchronous fallback | Existing overlap benefit, pacing and visibility costs. Change one opt-in path at a time afterward. |
| One memory or indirect-draw strategy at a time | Transfer, allocation, page-fault, and residency effects at matched scene intervals, with dropped work counted. |

Collect CPU frame and actual display interval median/P95/P99 with sample counts, CPU/GPU utilization, per-engine GPU activity and execution time, fence waits, and preparation and copy-back time. Record shader hits/misses, PSO load/create latency and queue age, upload and readback bytes, RAM/commit and VRAM usage, descriptor costs, and page faults. Track skipped draws and dispatches, pipeline refusals, device removals, image changes and scene progression. The current diagnostic frame log follows Present calls and the card meter is delayed. Use PIX timing captures and GPUView/ETW for CPU/GPU timeline attribution, and NVIDIA Nsight Graphics or AMD Radeon GPU Profiler where supported. Keep per-list timing runs separate from overlap comparisons. [Frame log][framelog] · [Present timestamps][present-log] · [Card meter][cardmeter] · [PIX timing][pix] · [GPUView][gpuview] · [Nsight GPU Trace][nsight] · [Radeon GPU Profiler][rgp]

Require preserved compute initialization, CPU-visible results, correct images and resource state, stable device behavior, and repeatable progression across scene boundaries. Validate across narrow-wave and wide-wave configurations when the optimization affects lanes. Keep captures, cache files and all title-derived artifacts local.

## Proposed investigation order

| Priority | Motivation and expected benefit | Prerequisite, risk and validation |
| --- | --- | --- |
| 0 · Measurement and correctness | Reproduce the AMD regression and separate first-use cost from steady rendering. | Pin scenes and settings, repeat on both vendors, and correlate output, submission and GPU timelines. |
| 1 · CPU–GPU execution | Identify useful overlap and reduce serialized CPU waiting or present jitter. | Validate default relay, synchronous control, async submit and copy paths independently. Gate on dependency order and retained lifetimes. |
| 2 · Memory | Reduce needless readback and snapshot traffic under realistic residency pressure. | Measure bytes and budgets. Exercise aliases, visibility boundaries and scene changes. |
| 3 · Shader translation | Preserve cross-wave correctness while reducing mask and communication overhead. | Use synthetic and differential checks before profiling registers, spills and occupancy. |
| 4 · Pipeline preparation | Reduce remaining first-use stalls and redundant compilation. | Verify runtime hits from both warm routes, then trial local manifests, critical scheduling and cache identity coverage. |
| 5 · Longer-term research | Establish whether partial graphics programs or DXR semantics fit selected workloads. | Require feature support or recoverable traversal semantics, then validate fidelity and performance independently. |

This is a contributor's investigation order. Revise it when matched traces or correctness failures identify a different priority.

### Primary reference index

- Console, shader and compiler contracts. [Xbox HLSL sample][gdk] · [DXIL][dxil] · [RDNA 2 ISA][rdna] · [NVIDIA warp model][warp] · [LLVM passes][llvm].
- Synchronization, transfers and residency. [D3D12 multi-engine synchronization][multi-engine] · [Fence-based resource lifetimes][fence-lifetime] · [GPU Upload Heaps][gpu-upload].
- Pipeline preparation. [D3D12 libraries][d3d-library] · [Unreal PSO precaching][epic-precache] · [Unreal bundled PSO caches][epic-bundle] · [Valve Fossilize][fossilize].
- API extensions. [Partial Graphics Programs][partial] · [Shader cache registration][asd-api] · [DXR][dxr] · [Vulkan graphics pipeline library][vk-gpl] · [Vulkan shader object][vk-so] · [Vulkan pipeline binary][vk-bin].

[alias-commit]: https://github.com/uiliannn/VItoriaBr/commit/8dd30ab472697c47b141c273c2a00f8ce1c00520
[arch]: https://github.com/uiliannn/VItoriaBr/blob/e7f9af536ed32411d3af2cd9c2a634020d069ea4/docs/architecture.md#L7-L27
[asd]: https://devblogs.microsoft.com/directx/advanced-shader-delivery-is-available-now-across-hardware/
[asd-api]: https://microsoft.github.io/DirectX-Specs/d3d/ShaderCacheRegistrationAPI.html
[async]: https://github.com/uiliannn/VItoriaBr/blob/e7f9af536ed32411d3af2cd9c2a634020d069ea4/crates/victory-gpu/src/queue.rs#L421-L508
[baseline]: https://github.com/uiliannn/VItoriaBr/commit/e7f9af536ed32411d3af2cd9c2a634020d069ea4
[budget]: https://github.com/uiliannn/VItoriaBr/blob/e7f9af536ed32411d3af2cd9c2a634020d069ea4/crates/victory-gpu/src/pipeline/make.rs#L94-L206
[bvh-gate]: https://github.com/uiliannn/VItoriaBr/blob/e7f9af536ed32411d3af2cd9c2a634020d069ea4/crates/victory-gpu/src/pipeline.rs#L305-L414
[cache]: https://github.com/uiliannn/VItoriaBr/blob/e7f9af536ed32411d3af2cd9c2a634020d069ea4/crates/victory-pipeline/src/cache.rs#L9-L74
[cardmeter]: https://github.com/uiliannn/VItoriaBr/blob/e7f9af536ed32411d3af2cd9c2a634020d069ea4/crates/victory-gpu/src/cardmeter.rs#L1-L26
[clamp-commit]: https://github.com/uiliannn/VItoriaBr/commit/31714deb2d70bb7c5a963658ad0faed0368526c6
[commands]: https://github.com/uiliannn/VItoriaBr/blob/e7f9af536ed32411d3af2cd9c2a634020d069ea4/apps/victory/src/main.rs#L171-L193
[copy-order]: https://github.com/uiliannn/VItoriaBr/blob/e7f9af536ed32411d3af2cd9c2a634020d069ea4/crates/victory-gpu/src/queue2.rs#L223-L283
[copy-queue]: https://github.com/uiliannn/VItoriaBr/blob/e7f9af536ed32411d3af2cd9c2a634020d069ea4/crates/victory-gpu/src/queue2.rs#L1-L90
[corrected]: https://github.com/uiliannn/VItoriaBr/commit/0b0074e0b2765e574c622e9a3de566f20f04135b
[d3d-create-library]: https://learn.microsoft.com/en-us/windows/win32/api/d3d12/nf-d3d12-id3d12device1-createpipelinelibrary
[d3d-library]: https://learn.microsoft.com/en-us/windows/win32/api/d3d12/nn-d3d12-id3d12pipelinelibrary
[descriptor]: https://github.com/uiliannn/VItoriaBr/blob/e7f9af536ed32411d3af2cd9c2a634020d069ea4/crates/victory-gpu/src/descriptor/host.rs#L808-L937
[descriptor-null]: https://github.com/uiliannn/VItoriaBr/blob/e7f9af536ed32411d3af2cd9c2a634020d069ea4/crates/victory-gpu/src/descriptor/host.rs#L988-L1010
[diff-tests]: https://github.com/uiliannn/VItoriaBr/blob/e7f9af536ed32411d3af2cd9c2a634020d069ea4/crates/victory-shader/src/emit/diff_tests.rs#L988-L1077
[dxil]: https://github.com/microsoft/DirectXShaderCompiler/blob/main/docs/DXIL.rst#introduction
[dxr]: https://microsoft.github.io/DirectX-Specs/d3d/Raytracing.html
[early]: https://github.com/uiliannn/VItoriaBr/blob/e7f9af536ed32411d3af2cd9c2a634020d069ea4/crates/victory-gpu/src/pipeline/make.rs#L118-L147
[epic-bundle]: https://dev.epicgames.com/documentation/unreal-engine/manually-creating-bundled-pso-caches-in-unreal-engine
[epic-precache]: https://dev.epicgames.com/documentation/unreal-engine/pso-precaching-for-unreal-engine
[fence]: https://github.com/uiliannn/VItoriaBr/blob/e7f9af536ed32411d3af2cd9c2a634020d069ea4/crates/victory-gpu/src/fence.rs#L78-L110
[fence-lifetime]: https://learn.microsoft.com/en-us/windows/win32/direct3d12/fence-based-resource-management
[fetch]: https://github.com/uiliannn/VItoriaBr/blob/e7f9af536ed32411d3af2cd9c2a634020d069ea4/crates/victory-gpu/src/host.rs#L1634-L1666
[fingerprint]: https://github.com/uiliannn/VItoriaBr/blob/e7f9af536ed32411d3af2cd9c2a634020d069ea4/crates/victory-shader/src/emit/mod.rs#L186-L211
[fixture]: https://github.com/uiliannn/VItoriaBr/commit/b37344f57f857bb2263972ec74bdb2296496cf34
[fork282]: https://github.com/fxceless/victory-fork/pull/282
[fork608]: https://github.com/fxceless/victory-fork/pull/608
[fossilize]: https://github.com/ValveSoftware/Fossilize
[framelog]: https://github.com/uiliannn/VItoriaBr/blob/e7f9af536ed32411d3af2cd9c2a634020d069ea4/crates/victory-gpu/src/framelog.rs#L1-L22
[gdk]: https://learn.microsoft.com/en-us/samples/microsoft/xbox-gdk-samples/hlslcompile/
[gpu-upload]: https://microsoft.github.io/DirectX-Specs/d3d/D3D12GPUUploadHeaps.html
[gpuview]: https://learn.microsoft.com/en-us/windows-hardware/drivers/display/using-gpuview
[group-barrier]: https://learn.microsoft.com/en-us/windows/win32/direct3dhlsl/groupmemorybarrierwithgroupsync
[group-sync]: https://github.com/uiliannn/VItoriaBr/blob/e7f9af536ed32411d3af2cd9c2a634020d069ea4/crates/victory-shader/src/emit/mod.rs#L124-L165
[import]: https://github.com/uiliannn/VItoriaBr/commit/dfd22d14cf1b7777510bf7a63605c41cc2416140
[inplace]: https://github.com/uiliannn/VItoriaBr/blob/e7f9af536ed32411d3af2cd9c2a634020d069ea4/crates/victory-gpu/src/inplace.rs#L57-L156
[intersect]: https://github.com/uiliannn/VItoriaBr/blob/e7f9af536ed32411d3af2cd9c2a634020d069ea4/crates/victory-shader/src/emit/intersect.rs#L1-L92
[ir]: https://github.com/uiliannn/VItoriaBr/blob/e7f9af536ed32411d3af2cd9c2a634020d069ea4/crates/victory-shader/src/ir.rs#L1-L16
[issue1]: https://github.com/uiliannn/VItoriaBr/issues/1
[issue13]: https://github.com/uiliannn/VItoriaBr/issues/13
[issue14]: https://github.com/uiliannn/VItoriaBr/issues/14
[issue16]: https://github.com/uiliannn/VItoriaBr/issues/16
[issue18]: https://github.com/uiliannn/VItoriaBr/issues/18
[issue4]: https://github.com/uiliannn/VItoriaBr/issues/4
[issue5]: https://github.com/uiliannn/VItoriaBr/issues/5
[late]: https://github.com/uiliannn/VItoriaBr/blob/e7f9af536ed32411d3af2cd9c2a634020d069ea4/crates/victory-gpu/src/host.rs#L1946-L2104
[library]: https://github.com/uiliannn/VItoriaBr/blob/e7f9af536ed32411d3af2cd9c2a634020d069ea4/crates/victory-gpu/src/pipeline/library.rs#L1-L46
[library-key]: https://github.com/uiliannn/VItoriaBr/blob/e7f9af536ed32411d3af2cd9c2a634020d069ea4/crates/victory-gpu/src/pipeline/library.rs#L501-L563
[llvm]: https://llvm.org/docs/Passes.html
[multi-engine]: https://learn.microsoft.com/en-us/windows/win32/direct3d12/user-mode-heap-synchronization
[nsight]: https://docs.nvidia.com/nsight-graphics/UserGuide/gpu-trace-activity.html
[nvidia-upload]: https://developer.nvidia.com/blog/optimizing-dx12-resource-uploads-to-the-gpu-using-gpu-upload-heaps/
[occupancy]: https://gpuopen.com/learn/occupancy-explained/
[opt]: https://github.com/uiliannn/VItoriaBr/blob/e7f9af536ed32411d3af2cd9c2a634020d069ea4/crates/victory-shader/src/emit/bitcode/opt.rs#L1-L58
[own-bits]: https://github.com/uiliannn/VItoriaBr/blob/e7f9af536ed32411d3af2cd9c2a634020d069ea4/crates/victory-shader/src/emit/own_bits.rs#L16-L39
[partial]: https://microsoft.github.io/DirectX-Specs/d3d/PartialGraphicsPrograms.html
[partial-retail]: https://devblogs.microsoft.com/directx/agilitysdk-1-620-retail-and-1-722-preview/
[pending]: https://github.com/uiliannn/VItoriaBr/blob/e7f9af536ed32411d3af2cd9c2a634020d069ea4/crates/victory-gpu/src/pipeline/make.rs#L27-L70
[pix]: https://learn.microsoft.com/en-us/windows/pix/timing-captures/timing-captures-overview
[pr10]: https://github.com/uiliannn/VItoriaBr/pull/10
[pr11]: https://github.com/uiliannn/VItoriaBr/pull/11
[pr12]: https://github.com/uiliannn/VItoriaBr/pull/12
[pr15]: https://github.com/uiliannn/VItoriaBr/pull/15
[pr6]: https://github.com/uiliannn/VItoriaBr/pull/6
[pr7]: https://github.com/uiliannn/VItoriaBr/pull/7
[pr8]: https://github.com/uiliannn/VItoriaBr/pull/8
[pr9]: https://github.com/uiliannn/VItoriaBr/pull/9
[present-async]: https://github.com/uiliannn/VItoriaBr/blob/e7f9af536ed32411d3af2cd9c2a634020d069ea4/crates/victory-gpu/src/queue.rs#L72-L162
[present-log]: https://github.com/uiliannn/VItoriaBr/blob/e7f9af536ed32411d3af2cd9c2a634020d069ea4/crates/victory-gpu/src/queue.rs#L1016-L1071
[priority]: https://github.com/uiliannn/VItoriaBr/commit/8cb509bc09f411049bca57e34ffd277aee0ed357
[psdb]: https://microsoft.github.io/DirectX-Specs/d3d/ExplicitPSDBs.html
[pso]: https://github.com/uiliannn/VItoriaBr/blob/e7f9af536ed32411d3af2cd9c2a634020d069ea4/crates/victory-gpu/src/pipeline/host.rs#L522-L688
[queue-waits]: https://github.com/uiliannn/VItoriaBr/blob/e7f9af536ed32411d3af2cd9c2a634020d069ea4/crates/victory-gpu/src/queue.rs#L163-L233
[rdna]: https://docs.amd.com/v/u/en-US/rdna2-shader-instruction-set-architecture
[readback]: https://learn.microsoft.com/en-us/windows/win32/direct3d12/readback-data-using-heaps
[readme-tuning]: https://github.com/uiliannn/VItoriaBr/blob/e7f9af536ed32411d3af2cd9c2a634020d069ea4/README.md#L105-L127
[relay-default]: https://github.com/uiliannn/VItoriaBr/blob/e7f9af536ed32411d3af2cd9c2a634020d069ea4/crates/victory-gpu/src/shadow.rs#L633-L657
[residency]: https://learn.microsoft.com/en-us/windows/win32/api/dxgi1_4/ns-dxgi1_4-dxgi_query_video_memory_info
[resource]: https://github.com/uiliannn/VItoriaBr/blob/e7f9af536ed32411d3af2cd9c2a634020d069ea4/crates/victory-shader/src/resource.rs#L1-L26
[rgp]: https://gpuopen.com/rgp/
[run86]: https://github.com/uiliannn/VItoriaBr/blob/e7f9af536ed32411d3af2cd9c2a634020d069ea4/crates/victory-gpu/src/host.rs#L1946-L1962
[series-s]: https://www.xbox.com/en-US/consoles/xbox-series-s
[series-x]: https://www.xbox.com/en-US/consoles/xbox-series-x
[shader-build]: https://github.com/uiliannn/VItoriaBr/blob/e7f9af536ed32411d3af2cd9c2a634020d069ea4/crates/victory-shader/build.rs#L34-L65
[shader-key]: https://github.com/uiliannn/VItoriaBr/blob/e7f9af536ed32411d3af2cd9c2a634020d069ea4/crates/victory-gpu/src/shader_cache.rs#L40-L116
[shader-objects]: https://github.com/uiliannn/VItoriaBr/blob/e7f9af536ed32411d3af2cd9c2a634020d069ea4/crates/victory-gpu/src/pipeline.rs#L59-L195
[shadow]: https://github.com/uiliannn/VItoriaBr/blob/e7f9af536ed32411d3af2cd9c2a634020d069ea4/crates/victory-gpu/src/shadow.rs#L106-L220
[snapshots]: https://github.com/uiliannn/VItoriaBr/blob/e7f9af536ed32411d3af2cd9c2a634020d069ea4/crates/victory-gpu/src/shadow.rs#L2337-L2366
[structure]: https://github.com/uiliannn/VItoriaBr/blob/e7f9af536ed32411d3af2cd9c2a634020d069ea4/crates/victory-shader/src/structure.rs#L1-L24
[sync]: https://github.com/uiliannn/VItoriaBr/blob/e7f9af536ed32411d3af2cd9c2a634020d069ea4/crates/victory-gpu/src/host.rs#L5309-L5596
[texture-budget]: https://github.com/uiliannn/VItoriaBr/blob/e7f9af536ed32411d3af2cd9c2a634020d069ea4/crates/victory-gpu/src/resource.rs#L1412-L1475
[texture-refresh]: https://github.com/uiliannn/VItoriaBr/blob/e7f9af536ed32411d3af2cd9c2a634020d069ea4/crates/victory-gpu/src/texture.rs#L352-L376
[threading]: https://github.com/uiliannn/VItoriaBr/blob/e7f9af536ed32411d3af2cd9c2a634020d069ea4/crates/victory-shader/src/emit/threading.rs#L1-L72
[translate-checks]: https://github.com/uiliannn/VItoriaBr/blob/e7f9af536ed32411d3af2cd9c2a634020d069ea4/apps/victory/src/translate.rs#L612-L651
[translate-warm]: https://github.com/uiliannn/VItoriaBr/blob/e7f9af536ed32411d3af2cd9c2a634020d069ea4/apps/victory/src/translate.rs#L155-L234
[victory1133]: https://github.com/Marvisak/VIctory/pull/1133
[victory2311]: https://github.com/Marvisak/VIctory/pull/2311
[vk-bin]: https://docs.vulkan.org/refpages/latest/refpages/source/VK_KHR_pipeline_binary.html
[vk-gpl]: https://docs.vulkan.org/refpages/latest/refpages/source/VK_EXT_graphics_pipeline_library.html
[vk-so]: https://docs.vulkan.org/refpages/latest/refpages/source/VK_EXT_shader_object.html
[warm]: https://github.com/uiliannn/VItoriaBr/blob/e7f9af536ed32411d3af2cd9c2a634020d069ea4/apps/victory/src/warm.rs#L21-L107
[warp]: https://docs.nvidia.com/cuda/volta-tuning-guide/index.html
[wave]: https://github.com/uiliannn/VItoriaBr/blob/e7f9af536ed32411d3af2cd9c2a634020d069ea4/crates/victory-shader/src/emit/wave.rs#L1-L15
[wave-select]: https://github.com/uiliannn/VItoriaBr/blob/e7f9af536ed32411d3af2cd9c2a634020d069ea4/crates/victory-shader/src/emit/mod.rs#L359-L603
[wide]: https://github.com/uiliannn/VItoriaBr/blob/e7f9af536ed32411d3af2cd9c2a634020d069ea4/crates/victory-gpu/src/shadow.rs#L1287-L1311
[workers]: https://github.com/uiliannn/VItoriaBr/blob/e7f9af536ed32411d3af2cd9c2a634020d069ea4/crates/victory-gpu/src/pipeline/make.rs#L222-L345
[ztrx20]: https://github.com/Zettrox/ztrx/pull/20
[ztrx30]: https://github.com/Zettrox/ztrx/pull/30
