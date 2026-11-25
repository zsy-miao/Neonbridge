# Comparative Performance Hypotheses for VItoriaBr

This review uses three related projects to identify measurements that could explain VItoriaBr's remaining shader, frame-time, and stability problems. It follows the architecture review in [VItoriaBr #19][architecture-review]. The comparison starts from fixed source revisions and distinguishes merged behavior, pending experiments, contributor observations, and hypotheses. The forks share history, so a feature or result appearing in multiple repositories may trace back to the same change or run.

## Comparison baseline

| Repository and pinned main | Evidence used | Limits |
| --- | --- | --- |
| [VItoriaBr `009856f`][br-main] | Target. Includes the descriptor wraparound correction after the revision in AMD issue #18, two shader warm-up routes, and the current victory-fork main in its ancestry. | #18 compared two older target revisions. Establish whether the symptom survives on this baseline. |
| [VIctory-Belobug-AMD `71652a2`][amd-main] | Radeon-focused report of shader queue separation, descriptor-allocation tails, indirect draw experiments, and alias visibility. | The README's 3–5 FPS observation uses one RX 9070 XT configuration, with approximations and unresolved phone, memory, and timeout problems. |
| [ZTRX `90b529f`][zx-main] | Same-family comparison for first-use PSO waits, renderer symptoms, and device diagnostics, with separate contributor reports and pending PRs. | The README's RTX 3050 results and later configurations use different cards and workloads. |
| [victory-fork `0b0074e`][fx-main] | Shared ancestry for async submission, copy queues, indirect draw windows, and frame/card instrumentation. Later PRs provide narrower diagnostic leads. | The current main is already incorporated. Unmerged PR observations are conditional on the contributor's revision and test setup. |

The primary target case is [VItoriaBr #18][br18]. On one RX 6500 XT, the reporter observed multi-second frames and black faces after updating from `18924c7` to `e7f9af5`, with shader warm-up completed. Several changes fall between those revisions, and the older [#13][br13] report differs in storage context. Check the two reported revisions and current main under matching conditions before associating the symptom with a specific commit or fix. [VItoriaBr sync][br-sync] · [Current descriptor correction][br-main]

## 1. First-use pipelines and missing producers

**Observed.** ZTRX #73 reported that a wider wait budget reduced pending-pipeline drops on a Radeon 780M in its diagnostic configuration, with other drops remaining. ZTRX #132 reported missing face or head geometry in its initial cold run and recovery on later runs. A subsequent run failed to save its pipeline library on exit. In Belobug, the author associated missing diffuse-light table production with a one-time draw skipped under cold pipeline contention. Its current source creates separate workers and a separate wait allowance for graphics classified as screen passes. Those classifications and timings describe that fork's experiment. [ZTRX #73][zx73] · [ZTRX #132][zx132] · [Belobug lighting observation][amd-light] · [Belobug pipeline scheduling][amd-pipeline]

**Target delta.** VItoriaBr already warms the translated shader cache, uses separate host PSO library entries, and prioritizes requested vertex graphics ahead of other requested and early items. Its shared wait allowance defaults to 500 ms per title frame. Warm DXIL can coexist with first-use host PSOs, and a timeout can skip a producer whose output is still required later. Pending victory-fork #955 reports a faster drain with more compilation resources alongside lower FPS while draining in one configuration. Measure queue age by phase before pursuing a different priority or worker allocation. [VItoriaBr warm][br-warm] · [Wait budget][br-make] · [Worker priorities][br-workers] · [victory-fork #955][fx955] · [D3D12 pipeline libraries][library] · [Unreal PSO precaching][epic]

**Hypothesis and decision.** A dependency-critical first-use pass may time out before producing data consumed on subsequent frames. Compare cold, shader-warm, and verified PSO-library-warm runs at the same transition. Correlate the first affected frame with requested, ready, refused and skipped PSOs, and with the producer's dispatch or draw and its output. Investigate loading-phase readiness or workload priority only if a missing producer explains the symptom. Deprioritize first-use scheduling if the producer runs and the data is already wrong, or if the same failure persists with verified PSO hits and no timed-out work.

## 2. Black faces can originate at several stages

**Observed.** Belobug #48 reports black albedo with placeholder bindings and zero feedback at an observed present on one warm AMD configuration. The imported victory-fork history already includes rechecking textures selected by index after upload when skin and hair lacked color. Pending victory-fork #927 reports a separate RTX 5080 case where face-animation buffers remained zero until packed 16-bit typed-buffer handling preserved the relevant representation. Animation, missing meshes, and surface color are different symptoms. VItoriaBr still has a documented floating-point interpretation shortcut in its `d16` formatted-buffer emitter. [Belobug #48][amd48] · [Imported texture recheck][fx-texture] · [victory-fork #927][fx927] · [VItoriaBr formatted-buffer lowering][br-d16]

**Hypothesis and decision.** The RX 6500 XT symptom could come from a missing pass, incorrect intermediate data, a stale texture binding, or downstream lighting. At the first affected frame, identify whether geometry is present, verify the producer and view format, then inspect sampled bindings and material output before lighting. Use a synthetic known-pattern comparison if the failing path actually exercises packed 16-bit buffer data. Favor a texture-visibility investigation if correct data arrives late or a placeholder persists, and favor translation correctness if the producer runs yet returns divergent values for matching inputs. If correct material data reaches lighting and only the final output is dark, investigate lighting separately. [RDNA 2 ISA][rdna] · [PIX GPU captures][pix-gpu]

The target still samples texture contents at selected offsets during rechecks. Pending victory-fork #956 reported that some mutations fell outside those windows in one RTX 5080 run involving missing HUD glyphs. For #18, compare the expected update with the sampled and final contents before attributing a stale texture to that coverage limit. [VItoriaBr texture sampling][br-texture-sample] · [victory-fork #956][fx956]

ZTRX #137 reports fewer world-entry stops in a GTX 1660 Wine trial of nonwaiting texture refresh. VItoriaBr already has that path and #18 enabled it. Treat existing lock behavior as a variable to compare on native Windows, with texture readiness and output checked in both cases. The Wine trial does not establish the origin of a Radeon rendering defect. [ZTRX #137][zx137] · [VItoriaBr texture refresh][br-texture]

**First controlled comparison for #18.** The VItoriaBr maintainer proposed changing only the card-buffer cap from the 6,144 MiB preset to 1,024 MiB. The target includes remaining buffer and render-target reserves in its bindless texture admission check. ZTRX main's corresponding check uses reported usage and budget without those reserves. A texture can therefore be deferred before actual usage reaches the OS budget. Compare per-frame admission refusals, pending upload age, budget and actual usage, paging events, transfer volume, and face output. Keep the texture budget override unset for that comparison. The cap alone gives no measured allocation or paging result. [Maintainer's proposed test][br18-comment] · [Target texture admission][br-vram] · [Target buffer reserves][br-reserve] · [ZTRX texture admission][zx-vram] · [DXGI memory budgets][residency]

## 3. CPU descriptor pressure and GPU indirect work

**Observed.** Belobug recorded repeated descriptor searches when slots were exhausted, and its current source bounds failed scans. VItoriaBr's current main already extends the bounded wraparound search past the cursor and has free-count enforcement, failed-size suppression, and diagnostic time and refusal counters. Belobug's README separately reports a substantial GPU-time change in one heavy scene with indirect clamps moved before a draw run. A later donor commit records that ordering was needed to remove flicker. VItoriaBr's imported draw-window path is a different mechanism. victory-fork withdrew its original 6.0 versus 8.9 FPS comparison across different scenes. The revised 145 versus 124 ms observation used one RTX 4070 run per configuration at similar draw counts and changed both windows and copy behavior. [Belobug allocator][amd-descriptor] · [VItoriaBr allocator][br-descriptor] · [Belobug indirect observation][amd-readme] · [Belobug clamp ordering][amd-clamp] · [victory-fork correction][fx-main]

**Hypothesis and decision.** Fragmented descriptor capacity could explain some CPU frame-time tails, while repeated indirect work could explain a different set of GPU-bound frames. Correlate descriptor request latency, refusals, and release progress with slow CPU frames. Separately compare GPU indirect phases with the current draw-window option on and off under the same workload, holding copy options constant. Count requested and executed work and require the same visible objects and frame-to-frame stability. A faster frame with dropped draws or flicker fails that comparison. Deprioritize descriptor pressure when slow frames have low descriptor time and few refusals. Deprioritize indirect work when GPU timing shows the time is elsewhere. [VItoriaBr draw windows][br-draw] · [PIX timing captures][pix] · [Radeon GPU Profiler][rgp]

## 4. Transfers, snapshot volume, and frame pacing

**Observed.** Relay, the ordered CPU submission worker, optional copy queues, and the associated frame instrumentation are already in the target's imported history. VItoriaBr's main uses relay stage 2 with late-wait stage 3 when unset. ZTRX #132 reported a different timing distribution after enabling relay. Its multi-second summaries had different draw counts, so the gain at a matched scene remains unknown. Pending victory-fork #916 reports unusually large snapshot comparison volume with the experimental BVH builder enabled on an RTX 5070 Ti. Its numbers are a diagnostic lead for that configuration. [VItoriaBr sync][br-sync] · [Target relay and late wait][br-relay] · [ZTRX #132][zx132] · [victory-fork #916][fx916]

**Hypothesis and decision.** Excess CPU preparation, overly broad possible-write ranges, or queue serialization can dominate different frames. The target can declare mapped ranges written for a shader with a store, which makes the #916 case relevant when that path is active. In a matched warm scene, compare the target defaults against its synchronous path, then vary the submit or copy queue option separately. Observe declared versus changed bytes, snapshot and compare time, upload/readback bytes, queue gaps, fence waits, and display intervals. Keep builder-off measurements separate from any optional builder-on investigation. Narrow the investigation to write coverage only if declared or compared bytes materially exceed changes on the slow path. Prioritize overlap only if hardware queues idle while dependent work is ready. A shift from CPU copying to a longer GPU critical path does not improve frame pacing by itself. [Target write declaration][br-write] · [Target copy diagnostics][br-copy] · [D3D12 queue synchronization][queues] · [PIX timing captures][pix]

## 5. Stability under residency and compiler pressure

**Observed.** Belobug #26 reports four RX 7700 XT loading runs that ended in device removal, with DRED reports naming recently freed allocations and usage near the card limit. A victory-fork build with an additional PR ran farther on the same machine under different conditions. Belobug #46 reports a clean main run on a different RX 9070 XT configuration. ZTRX #91 reports a device hang during heavy pipeline creation and an incompatible library across adapters. Belobug main also has handling for failed or hung driver pipeline creation. VItoriaBr #5 and pending ZTRX #109 report a matching AMD driver fault signature on different hardware and Windows configurations. That makes driver version and PSO identity useful comparison fields. [Belobug #26][amd26] · [Belobug #46][amd46] · [Belobug driver handling][amd-driver] · [ZTRX #91][zx91] · [VItoriaBr #5][br5] · [ZTRX #109][zx109]

**Hypothesis and decision.** A resource lifetime issue or residency spike could explain a subset of removals. A repeatable exception inside driver PSO creation calls for a separate compiler investigation. Record adapter, driver, selected cache, queue and last workload, DRED removal reason and fault context, dynamic local/nonlocal budgets, and private commit. Compare one configuration at a time against the same run milestone. A recently freed allocation in DRED narrows the investigation. Establish the identity of the used resource and fence order before attributing a lifetime fault. Investigate compile pressure if the exception repeats at pipeline creation with stable budgets, and residency if pressure and stalling precede failures or recover when headroom is restored. Keep saves and diagnostic captures separate and local. [VItoriaBr DRED path][br-dred] · [Microsoft DRED guidance][dred] · [DXGI memory budgets][residency] · [Fence-based resource lifetimes][lifetimes]

## 6. Wave and helper-lane correctness before timing

**Observed.** Belobug's README attributes visible triangle-edge artifacts to handling of quad helper lanes. The helper-lane and quad-read changes already appear in VItoriaBr history. This matters when comparing the forks across AMD and NVIDIA: equivalent source behavior can still have different latency or resource usage, and existing visual approximations can change the workload. Keep the state of lighting substitutions, skipped passes, and optional shader or indirect modes in the run record. [Belobug report][amd-readme] · [VItoriaBr quad-read commit][br-quad] · [AMD occupancy guidance][occupancy] · [D3D12 group barrier contract][barrier]

**Hypothesis and decision.** If a remaining shader divergence reproduces under identical inputs, a correctness-preserving lowering could remove redundant work. Establish agreement for masks, helper lanes, cross-wave values and visible memory before profiling spills, occupancy, or barriers. Stable synthetic output plus a shorter hardware GPU interval for the same pass would support further investigation. If the donor's apparent speed requires different lighting, fewer dispatches, or an approximation, keep it as a diagnostic comparison.

## Suggested measurement order

1. Reproduce #18 on the two reported revisions and current main using the same title profile, saved starting point, capture interval, adapter, driver, display settings, storage path, and renderer options. Repeat each configuration. If reproduced, compare the existing [pre-sync][br-quad], [post-sync][br-sync], and [post-MIMG][br-mimg] revisions to narrow the range. Record result and sample count.
2. Establish distinct cold, shader-warm, and verified PSO-library-warm states. Record driver and filesystem cache context. Use the same adapter and driver for each compatible cache and retain a control copy.
3. Pair frame-time median, P95 and P99 with display intervals, GPU execution and idle time, descriptor pressure, queued/skipped work, transfers, and memory budgets. Existing per-list GPU timing alters submission behavior. Collect it separately from overlap comparisons.
4. Compare identical checkpoints for visual results and persistent compute outputs. Keep a run that reaches the next checkpoint but diverges visually or loses work in the failure set.

This review records observations and experimental questions. Source revisions, reported symptoms, and the proposed falsifiers are the basis for deciding whether each line of investigation warrants work. [VItoriaBr asset policy](asset-policy.md)

[amd-clamp]: https://github.com/Belobug/VIctory-Belobug-AMD/commit/76387c2dea079eb1d31f80402471cb4f55b34d2b
[amd-descriptor]: https://github.com/Belobug/VIctory-Belobug-AMD/blob/71652a2c15a0da68cd801890e1f71109670f9ca5/crates/victory-gpu/src/descriptor/host.rs#L767-L841
[amd-driver]: https://github.com/Belobug/VIctory-Belobug-AMD/blob/71652a2c15a0da68cd801890e1f71109670f9ca5/crates/victory-gpu/src/pipeline/library.rs#L109-L177
[amd-light]: https://github.com/Belobug/VIctory-Belobug-AMD/commit/18e57db87d52c7ab4dce6e9c3435d197b218d80c
[amd-main]: https://github.com/Belobug/VIctory-Belobug-AMD/commit/71652a2c15a0da68cd801890e1f71109670f9ca5
[amd-pipeline]: https://github.com/Belobug/VIctory-Belobug-AMD/blob/71652a2c15a0da68cd801890e1f71109670f9ca5/crates/victory-gpu/src/pipeline/make.rs#L107-L261
[amd-readme]: https://github.com/Belobug/VIctory-Belobug-AMD/blob/71652a2c15a0da68cd801890e1f71109670f9ca5/README.md#L12-L68
[amd26]: https://github.com/Belobug/VIctory-Belobug-AMD/issues/26
[amd46]: https://github.com/Belobug/VIctory-Belobug-AMD/issues/46
[amd48]: https://github.com/Belobug/VIctory-Belobug-AMD/issues/48
[architecture-review]: https://github.com/uiliannn/VItoriaBr/pull/19
[barrier]: https://learn.microsoft.com/en-us/windows/win32/direct3dhlsl/groupmemorybarrierwithgroupsync
[br-copy]: https://github.com/uiliannn/VItoriaBr/blob/009856f0c950c1ecdddba4f0557f263999b01ef0/crates/victory-gpu/src/copystat.rs#L1116-L1155
[br-d16]: https://github.com/uiliannn/VItoriaBr/blob/009856f0c950c1ecdddba4f0557f263999b01ef0/crates/victory-shader/src/emit/lower.rs#L9886-L9912
[br-descriptor]: https://github.com/uiliannn/VItoriaBr/blob/009856f0c950c1ecdddba4f0557f263999b01ef0/crates/victory-gpu/src/descriptor/host.rs#L1070-L1126
[br-draw]: https://github.com/uiliannn/VItoriaBr/blob/009856f0c950c1ecdddba4f0557f263999b01ef0/crates/victory-gpu/src/draw.rs#L143-L191
[br-dred]: https://github.com/uiliannn/VItoriaBr/blob/009856f0c950c1ecdddba4f0557f263999b01ef0/crates/victory-gpu/src/host.rs#L5612-L5653
[br-main]: https://github.com/uiliannn/VItoriaBr/commit/009856f0c950c1ecdddba4f0557f263999b01ef0
[br-make]: https://github.com/uiliannn/VItoriaBr/blob/009856f0c950c1ecdddba4f0557f263999b01ef0/crates/victory-gpu/src/pipeline/make.rs#L94-L205
[br-mimg]: https://github.com/uiliannn/VItoriaBr/commit/f06ee8b4ac80e5eedd79fdd9fa56763ccca84810
[br-quad]: https://github.com/uiliannn/VItoriaBr/commit/2b3c339ef155d5798312d653db0d1a4fa0eee371
[br-relay]: https://github.com/uiliannn/VItoriaBr/blob/009856f0c950c1ecdddba4f0557f263999b01ef0/crates/victory-gpu/src/host.rs#L1993-L2104
[br-reserve]: https://github.com/uiliannn/VItoriaBr/blob/009856f0c950c1ecdddba4f0557f263999b01ef0/crates/victory-gpu/src/shadow.rs#L142-L219
[br-sync]: https://github.com/uiliannn/VItoriaBr/commit/dfd22d14cf1b7777510bf7a63605c41cc2416140
[br-texture]: https://github.com/uiliannn/VItoriaBr/blob/009856f0c950c1ecdddba4f0557f263999b01ef0/crates/victory-gpu/src/texture.rs#L350-L376
[br-texture-sample]: https://github.com/uiliannn/VItoriaBr/blob/009856f0c950c1ecdddba4f0557f263999b01ef0/crates/victory-gpu/src/texture.rs#L1210-L1246
[br-vram]: https://github.com/uiliannn/VItoriaBr/blob/009856f0c950c1ecdddba4f0557f263999b01ef0/crates/victory-gpu/src/resource.rs#L1408-L1475
[br-warm]: https://github.com/uiliannn/VItoriaBr/blob/009856f0c950c1ecdddba4f0557f263999b01ef0/apps/victory/src/warm.rs#L21-L105
[br-workers]: https://github.com/uiliannn/VItoriaBr/blob/009856f0c950c1ecdddba4f0557f263999b01ef0/crates/victory-gpu/src/pipeline/make.rs#L222-L344
[br-write]: https://github.com/uiliannn/VItoriaBr/blob/009856f0c950c1ecdddba4f0557f263999b01ef0/crates/victory-gpu/src/draw.rs#L1009-L1024
[br13]: https://github.com/uiliannn/VItoriaBr/issues/13
[br18]: https://github.com/uiliannn/VItoriaBr/issues/18
[br18-comment]: https://github.com/uiliannn/VItoriaBr/issues/18#issuecomment-7164281178
[br5]: https://github.com/uiliannn/VItoriaBr/issues/5
[dred]: https://learn.microsoft.com/en-us/windows/win32/direct3d12/use-dred
[epic]: https://dev.epicgames.com/documentation/unreal-engine/pso-precaching-for-unreal-engine
[fx-main]: https://github.com/fxceless/victory-fork/commit/0b0074e0b2765e574c622e9a3de566f20f04135b
[fx-texture]: https://github.com/fxceless/victory-fork/commit/a803c87c48078e57b296e06e08146d7ceca44530
[fx916]: https://github.com/fxceless/victory-fork/pull/916
[fx927]: https://github.com/fxceless/victory-fork/pull/927
[fx955]: https://github.com/fxceless/victory-fork/pull/955
[fx956]: https://github.com/fxceless/victory-fork/pull/956
[library]: https://learn.microsoft.com/en-us/windows/win32/api/d3d12/nf-d3d12-id3d12device1-createpipelinelibrary
[lifetimes]: https://learn.microsoft.com/en-us/windows/win32/direct3d12/fence-based-resource-management
[occupancy]: https://gpuopen.com/learn/occupancy-explained/
[pix]: https://learn.microsoft.com/en-us/windows/pix/timing-captures/timing-captures-overview
[pix-gpu]: https://devblogs.microsoft.com/pix/gpu-captures/
[queues]: https://learn.microsoft.com/en-us/windows/win32/direct3d12/user-mode-heap-synchronization
[rdna]: https://docs.amd.com/v/u/en-US/rdna2-shader-instruction-set-architecture
[residency]: https://learn.microsoft.com/en-us/windows/win32/api/dxgi1_4/ns-dxgi1_4-dxgi_query_video_memory_info
[rgp]: https://gpuopen.com/rgp/
[zx-main]: https://github.com/Zettrox/ztrx/commit/90b529f878a470fec9bf07b7be316bc769a12232
[zx-vram]: https://github.com/Zettrox/ztrx/blob/90b529f878a470fec9bf07b7be316bc769a12232/crates/victory-gpu/src/resource.rs#L1365-L1384
[zx109]: https://github.com/Zettrox/ztrx/pull/109
[zx132]: https://github.com/Zettrox/ztrx/issues/132
[zx137]: https://github.com/Zettrox/ztrx/pull/137
[zx73]: https://github.com/Zettrox/ztrx/issues/73
[zx91]: https://github.com/Zettrox/ztrx/issues/91
