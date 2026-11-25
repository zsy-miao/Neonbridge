> Français : [demarrage-leonida-2026-11-23.md](../fr/recherche/demarrage-leonida-2026-11-23.md)

# Booting 1.0.2.1 under Leonida, 23 November 2026: stops, black frames, static findings

State as of the evening of 23 November 2026. This note files the research of that day (Claude and Codex, on the
maintainer's machine) on the boot of build 1.0.2.1 under [Leonida](https://github.com/ItsNotRexed/Leonida)'s
`gta6rt`, from the first stop to the frame loop. It continues [first-boot-1021.md](first-boot-1021.md), which stops at
the first six stops and the work list for AnyXboxX. Neighbours in general: [neighbors.md](neighbors.md); Leonida's
GTA 6 notes: [leonida-gta6.md](leonida-gta6.md).

**Rule 1 of [AGENTS.md](../../AGENTS.md) applies, and more strictly than in the first note.** This note holds
vtable slot numbers, flag values, HRESULTs, public SDK type names, PR numbers, counts, times and memory sizes. It
holds no address or offset of the executable, no disassembled or decompiled code, no byte, no string, no file or
shader name of the package, no raw log and no hash of a shader. Descriptor layouts are given as field positions of
public SDK structures, never as code. The local reports behind it (REA, Ghidra and capstone by Claude, Codex's
reports, launch logs) stay outside the repository, as do the names of their files.

**Three levels, in every table.**

| Label | Meaning |
| --- | --- |
| measured | observed on a run, or read from a tool's output by a method anyone can repeat. Says who measured. |
| deduced statically | read from the executable without running it (targeted decompilation, disassembly). Direct calls only, so it is a lower bound; no reachability is proven |
| assumed | inferred from the two above, from a name, or true on one configuration only |

Measurements are ours unless a name is given. Names are GitHub logins of [Leonida](https://github.com/ItsNotRexed/Leonida)
participants; their numbering of the stops is not ours unless said.

## 1. Set-up of the measurements

| Item | Value | Status |
| --- | --- | --- |
| Machine | Windows 11 (build 26300), Core i7-13700KF, RTX 4070 SUPER (driver 610.88), native, no Wine | measured |
| Runtime | `gta6rt`, MSVC RelWithDebInfo, `LEONIDA_D3D12=vulkan` from the second pass on | measured |
| Executable | the 1.0.2.1 build already described (SHA-256 starting `e95020b2`), own copy, package read-only | measured |
| Stacks | Leonida `main` at various dates (`1cc6e3c1`, then `ec0e01c5`, then `dde82f27`) with open pull requests merged locally, conflicts resolved by hand, **only `gta6rt` built, the PRs' own tests not run** unless a PR comment says so | measured |
| Later base | the maintainers' forge `main` (`2ac38583`, contains #7759), journal in issue #8851 | measured |
| Shaders | `make_game --stages shaders`: 26,503 translated in 32 min on one stack (wave64 tiers: A 236, B 7, C 425 for VS, PS, GS) | measured |
| Environment variables of the loop runs | `LEONIDA_ARENA_MB=4095` (otherwise `E_OUTOFMEMORY`), `LEONIDA_STUB_PIPELINES=1` for some runs, `LEONIDA_SKIP_INT3=1` for one in-image `int3` | measured |

Ordinary test results at rest on this machine, main `dde82f27`: ctest 268 of 272. The four failures are
`d3d12_vulkan_device` (a `tiles == 1` check), `d3d12_indirect_dispatch_count`, `d3d12_depth_resolve` and
`d3d12_indirect_rt`; they fail the same way on main and are not caused by the PRs we tested. Without SDL3 the
GameInput shim is not built (ctest reports `gameinput` as disabled): `-DLEONIDA_SDL3_FETCH=ON` builds it. measured.

## 2. The measured chain, stop by stop

Each row is the stop met after adding the pull request of the previous row to the local stack. "t" is wall time
to the stop, "peak" is the peak working set. Numbers 1 to 6 are those of [first-boot-1021.md](first-boot-1021.md);
numbers 13 to 19 are KkelebekK's (Linux, Wine, #6910). Pull requests belong to the people named; "ours" marks the
ones we wrote.

| Stop | What the title does there | Cleared by | t (s) | Peak (MB) |
| --- | --- | --- | --- | --- |
| 1, 2 | `D3D12ConfigureVirtualMemoryX` absent, then `XMemVirtualAlloc` of the init refused | #6792 (undeadhuman37), then the attribute-word forms of #6709 (Stalkerino), #4531 (labj1987), #6919 (NiiRoZz) | 6.3 to 8.3 | 238 |
| (log mode) | `XErrorReport` not exported by the shim: `WinMain` takes the failure branch | #6582 (Kam471) | 4.9 to 7.2 | 238 |
| 24 | `DStorageGetFactory` unimplemented | #4348 (mariuslozo-boop) with #4313 (TLS index) | 3.1 to 6.5 | 237 |
| (host) | access violation in the host after `XMemAllocatePhysicalPages` (0x280 requested, 11,360 pages), device slot 26 | #6964 (sennecools); the Vulkan backend raises the peak from 395 to 687 MB | 7.0 to 9.8 | 395, then 687 |
| 4 | device slot 48, queue `Flags` `0x2`, `0x3` | #6954 (Pipodi) | 7.4 | 687 |
| 5 | device slot 73, console list IID | #7076 (NiiRoZz), with #6953 (Pipodi) and #2490 (NolanCotter). #6953 and #2490 alone left the stop | 7.0 to 8.2 | 687 |
| after 6 | device slot 133 `E_NOTIMPL` "beyond the PC vtable" | #7075 (NiiRoZz) | 6.8 to 7.1 | 687 |
| 13 | descriptor heap slot 9, `GetCPUDescriptorHandleForHeapStart` | #7021 (Khuraie); the title then goes through slots 17, 18, 19 | 6.5 | 687 |
| (host) | host write access violation at the first slot 70 call | #7066 (tedin7) | 6.5 to 6.9 | 687 |
| (host) | read access violation at address -1, first slot 25 call | #7110 (tedin7). Merge note: it and #6964 both define the helper `proxy_get_custom_heap_properties`; together the file does not compile (`C2440`) | 7.1 | 687 |
| 16 | slot 127 `E_NOTIMPL`, then `int3` | #7052 (tedin7) | 6.9 to 7.0 | 687 |
| 17 | budget test after the render targets | #7235 (NiiRoZz): 33,423,360 bytes per target on this GPU. #7115 (slot 61, mrp1no) changes nothing here because the stop comes before slot 61 | 6.7 to 6.9 | 687 |
| 18 | uncaught C++ exception from the package's BVH builder DLL | #5680 (zzEmmazz, closed) plus the 52 embedded programs translated by hand into a work folder; #7180 (KkelebekK, zlib DirectStorage) changes nothing before it | 6.6, then 31.9 in stop mode | 687, then 1,091 |
| 19 | `int3` after device slots 39, 27, 41 | local stand-ins (section 2.2) | 7 (log mode) | 686 |

Our #6279 (narrow MUBUF stores) is byte-identical on the 52 programs to what main already produces thanks to
#1864 (LouisRiverstone): it changes nothing on the boot path (measured, comment on #6279).

All rows above are **measured**. The causes in the "what the title does there" column are measured for the first
six (see the first note) and deduced from the call site for the later ones.

### 2.1 Cross-checks of other people's pull requests (same machine, same stack)

We posted measured comments, as tester, on the pull requests below. They do not change the table, they add
independent runs. All of them end with the same four ctest failures of section 1.

| PR | Result | Note |
| --- | --- | --- |
| #6935 (Kam471) | queue `Flags` measurement | the three queues are created once `Flags &= 0x1` |
| #7006 | pass, with a note | without `LEONIDA_FFX_PC=1` the upscaler flag hooks nothing, contrary to the PR text; the picker is never reached on that base |
| #6986 | pass | 154 `xgameruntime_test` checks |
| #8003 and #7896 (Spacecer2) | #8003 pass, 342 checks, `GameInput.dll!#1` binds to the shim | the two add the same table row: a merge keeps it twice, the second to land must drop its row |
| #8033, #8055, #8043, #8023, #7937 | pass on NVIDIA (the previous checks were on AMD or GTX 1070) | #8033: nothing fails without the change, a small teardown test would pin it. #8043: 601 state cases compared with the host, 0 differ. #7937: 119 display modes on slot 21 |
| #7969 (on #7490) | the commit passes, the stack fails `queue_proxy` 3 of 3 | the failure comes from #7490 (tedin7), as sgonzalez-glitch reported |
| #7095 | pass (`inline_scan`: 18 tests) | see the limits in section 5.5 |

### 2.2 Local stand-ins, never published

The third pass (frame loop) used private patches to find the next stop. They are listed so nobody reads the
results below as the effect of public pull requests alone.

| Stand-in | Effect | Status |
| --- | --- | --- |
| list slot 139 answers like `Reset` | the title's list reset works (the same as #2490's idea) | measured |
| slot 133 state word read as a barrier layout; depth-stencil with UAV created without the UAV flag; fallback to a committed resource when the page import of #7218 fails | passes the first `int3` after slot 133 | measured |
| `LEONIDA_SKIP_INT3=1` | the title continues past one in-image `int3` that follows slot 25 | measured |
| `TF_LENIENT` (what #7007 intended) | a list with an invalid draw is still submitted | measured |

## 3. From the first draw to the frame loop

The same machine, native, Vulkan backend. The base is Leonida `main` `dde82f27` plus the boot-path PRs (#7201,
#7323, #7759, #7286, #7290, #7111, #7403, #7417, #7529, #7581, #6891, #7685, #7356, #7880, #7448, #7218, #7949,
#6885, #6981), plus the stand-ins of section 2.2. The runs are numbered in our journal; the table keeps what bears on
the result.

| Run | Result | Time (s) | Peak |
| --- | --- | --- | --- |
| stubs only (no translated shaders) | 4 `PresentX`, then a host access violation (null write); the arena is full without `LEONIDA_ARENA_MB` | 40 to 110 | 2 to 6 GB |
| with the translated shaders | 300 s loop; 53,830 pipelines created, 602 refused; at least 16 `PresentX` (about one every 10 s, the rest is compilation); 0 call of device slot 55 | 307 | 13.4 GB |
| lists executed | 0 draw: the frame list fails `Close`, so is never submitted; window white | 224 | 14 GB |
| lenient lists | `Close`: 661 ok, 0 failures; 17 batches executed with a clear and two rectangle-list draws | 200 | not recorded |
| with the swap chain checks | `PresentX` fails 16 of 16 with `E_INVALIDARG` (the frame token is never issued); at least 2,048 pipelines refused "as AS" (no VS, no MS) | 150 | not recorded |
| with the frame tokens of #7685 (Cattn) | `PresentX` succeeds; the window, made visible by hand, is 100 percent black | 160 | not recorded |
| type 24 read as VS (local, since disproved, section 6) | refusals 602 to 219 on two different stacks, then 3,140 to 219 with the stub option, see section 6 | 160 | not recorded |

Other stacks, same machine:

| Stack | Result | Status |
| --- | --- | --- |
| main `dde82f27` plus #7201 #7323 #7759 #7286 #7290 #7356 #7880 #7448 #7218 #7949 #6885 #6981, list slot 139 as `Reset` | 4 `PresentX` (R8G8B8A8 3840 by 2160, render target and UAV, double buffered), then an access violation. #7218's import of the slot 133 buffers onto the title's pages works on NVIDIA but needs `LEONIDA_ARENA_MB=4095`. Under `LEONIDA_STUB_PIPELINES` every draw is refused (pipeline 0) | measured |
| base `rt5-integration` (tibzejoker) plus #7759 plus #8815 | without slot 57 served, the title loops on slot 57: `CreateLifetimeTracker` refuses more than 3,600 calls in 5 minutes, 0 call of slot 55, 0 `PresentX`. With #8890 (RipcuBAN: slot 57 returns `E_INVALIDARG` for lack of `fspx_gfx`) the loop advances: more than 3,300 `CreatePipelineState` succeed in 5 minutes. Without the `AcpHal` shim (#6885, #6981 by Jadiac5) an access violation follows `ApuCreateHeap` | measured |
| maintainers' forge `main` `2ac38583` | `gta6rt` builds; the shader stage was still running at the end of the day (estimated 1 h 30). No frame result yet | measured (build), pending (runs) |

Measured by others, on forge `main` once slot 55 is in (#7759): Yoshi-OOF counts 41,545 successful derivations once
the slot 57 base objects are kept. lines9686-rgb and sh1shu1 run the same frame loop and give the two Vulkan
messages of section 5.6 and the mesh shader A/B of section 5.3. The title draws on forge `main`; the images stay
black (maintainer's report and issue #8851).

### 3.1 Pipeline compilation

**Measured** ([our comment on Leonida #7231](https://github.com/ItsNotRexed/Leonida/issues/7231#issuecomment-7107629416)): the 52 pipelines of the package's BVH builder take 2.41 s to
27.69 s on the first run (cold, about 25 s), peak 1,091 MB, then 0.3 to 0.7 s afterwards (under 1 s with the driver's
pipeline cache warm), peak 686 to 687 MB. RTX 4070 SUPER, driver 610.88. The 31.9 s of stop 18 in section 2 is the
same cold case in stop mode. The 26,503 translated shaders and the 53,830 pipelines of the
300 s run above come from different runs and must not be divided into each other.

## 4. The four measured causes of the black frames

Native, NVIDIA, frame loop. Each cause was measured by switching it on or off; the table gives what clears it and
who wrote that.

| # | Cause | Measurement | Clears it | Status |
| --- | --- | --- | --- | --- |
| 1 | `PresentX` fails with `E_INVALIDARG` | 16 of 16 calls fail while the device has no frame token | #7685 (Cattn): device-side `WaitFrameEventX` and the pacer share one token. With it `PresentX` returns success | measured |
| 2 | The window is never visible | `IsWindowVisible` is 0 for the whole run; the title never shows its window | local `TF_SHOW` only; no public PR identified | measured |
| 3 | The frame list never reaches the queue | a draw or dispatch is recorded without a pipeline, so `Close` returns `E_INVALIDARG` and the list is not submitted | #7007 (lines9686-rgb, closed) together with #7417 (rafaelpssz, `LEONIDA_STUB_PIPELINES`): with the lenient mode, `Close` succeeds 661 times of 661 | measured |
| 4 | Pipelines refused at creation | at least 2,048 device slot 86 streams arrive with subobject type 24, no VS and no MS, a pixel shader and a triangle topology; they were refused as amplification shaders | taking type 24 as the mesh shader (section 5.3, sh1shu1 and lines9686-rgb) | measured; the fix is the one corrected after our wrong reading, section 6 |

After causes 1 to 3, a triangle draw (topology 4, count 3, one instance) still has no pipeline: it needs a title
vertex program that is embedded and not yet translated (#7746 by Faterek). Codex checked two traces and found the
same refused draw with pipeline 0 and root signature 1; the same lines show two recorded draws with topology 17
(rectangle lists), and `PresentX` returning 0. This confirms the symptom, not the identity of the shader nor a single
cause of the black screen (static reading by Codex, measured counts).

## 5. Static findings

All of this is **deduced statically**, by Claude (REA 6.10.1, capstone, targeted Ghidra decompilation) and by Codex
(Ghidra, capstone, a synthetic C++ test), on our own copy. Slot numbers are the vtable index of the console device,
list or queue. Where Codex gives a limit, the limit is kept.

### 5.1 A trap in the HRESULT test (device loss)

| Item | Finding |
| --- | --- |
| Where | a common helper tests the result of 53 call sites after a graphics call. It is not a plain sign test |
| What | the HRESULTs `0x887A0005`, `0x887A0006`, `0x887A0007` and `0x887A0020` (the DXGI device-removed family: removed, hung, reset, driver internal error) lead to a function that has no return path and ends in an `int3`. Other negative HRESULTs are tolerated. The sites, by their argument shapes, include the slots called 50, 132 and 119 in crash-dump style |
| Reading | no `ret` in that function, all branches reach the `int3` (capstone). The "crash-dump" role comes from argument shapes only |
| Consequence | a layer that answers a device error with one of these four values ends the title. It should log the reason and, where it must fail, return another negative HRESULT. Not described publicly before. No fix proposed, as the coordinator asked |
| Status | deduced statically; 53 callers is a count of direct calls |

### 5.2 Descriptor layouts of device slots 55, 57, 86, 108, 70, 127

Layouts are field positions in the arguments, matched to public SDK structures where Codex or we could match them.
Nothing says that a field of unknown role is a size or a pointer.

| Slot | Role | Shape | Status |
| --- | --- | --- | --- |
| 55 | `CreateDerivedPipelineState(base PSO, n, records, riid, out)` | `n` from 1 to 3 in the static reading, 1 to 4 in #7759; records of 0x14c bytes, a 32-bit tag then a payload. Tags 0 (depth-stencil), 1 (rasterizer), 2 (blend) and 5 (stencil read and write mask bytes) read from the title's writes; tag 3 (sample mask, TomBox) is served and never written; tag 5 is **assumed**, not measured at run time | deduced; assumed for tag 5 |
| 57 | `DeserializeGraphicsPipelineStateX` (name from #8890) | block of 0x108 bytes: a root signature at the start, two object pairs, then seven blocks of 0x20 bytes `{pointer A, size A, pointer B, size B}`; index 0xFFFFFFFF means absent; the result is tested by sign through the trap of 5.1. The first field is very likely a root signature: it comes from device slot 16 with the PC `ID3D12RootSignature` IID, either created or taken from a cache. The role of the pairs is **assumed** (stage bytecode and size is the reading that fits) | deduced; assumed for the pairs |
| 86 | `CreatePipelineState` with a stream | `{u64 0x2b0, stream pointer}`, the PC `D3D12_PIPELINE_STATE_STREAM_DESC` shape (forwarded to PC slot 47 in #7201, justr4mzi). Subobject types 0 to 5 (root signature, VS, PS, DS, HS, GS) at fixed positions, then 8 (blend), 9, 10, 0x15 (depth-stencil 1), 0x11 (sample desc), 0xc (input layout), 0xe, 0xf (render target formats), 0x10 (depth format). Mesh form: the second subobject carries type 0x18 and the fifth 0x17. A failure here returns quietly, no `int3` (measured on a run: the title keeps a null pointer). Called 6 times in a row after stop 21 | deduced; the quiet failure measured |
| 108 | committed-resource creation with an alignment hint | `(return, device, 1, 1, resource description changed, info)`. The title asks for 0x1000 alignment first when the size is under 0x80008 and `flags & 3 == 0`, then falls back to 0; an `int3` follows a non-zero high dword of the size. Already described publicly by KkelebekK and justr4mzi | deduced |
| 70 | create a shader resource view | first argument matches a buffer `D3D12_RESOURCE_DESC` at the fields checked (dimension 1, height 1, depth 1, mip 1, row-major layout, width from a 12-bit field times a 32-bit count); a texture branch writes dimension 3 and the texture fields. Second argument matches `D3D12_SHADER_RESOURCE_VIEW_DESC` (40 bytes) followed by one console QWORD at +0x28, taken from the container. The QWORD is not shown to be a GPU address. A cache write-back loop follows on the third argument, so the third is memory the title owns. Six direct callers; indirect callers not covered | deduced; strongly consistent |
| 127 | `CreateCommittedResource3` shape | heap properties as five DWORDs `{DEFAULT, ?, ?, 1, 1}`, flags 8, a description, then an initial layout word, the PC `ID3D12Resource` IID and an output. The title tests the result by sign; a failure frees the object and returns null to the caller that ends in the `int3` of stop 16 | deduced |
| 35 (queue) | `ExecuteCommandLists` (console) | consumed by the frame loop, which reads a ring of 0x50-byte entries | deduced; already public for 78, 79, 80 |
| 80, 81 (device) | frame events (`(dev, 2, 0xffffffff, 0, 0, &token)` and `(dev, 0, 0, 0, 2, &obj)`); slot 81 asserts if negative | slot 81 is `GetFrameEventsX` in #8869 | deduced |

### 5.3 Subobject types 23 and 24: what we got wrong and the correction

| Reading | Who | Evidence | Status |
| --- | --- | --- | --- |
| 24 = amplification shader, 23 = mesh | us, at first | the retyping of the second and fifth subobjects was read as AS and MS | **wrong** |
| 24 = vertex shader, 23 = hull shader | TreXito (#8186, static), then us (#8815) | those PSOs carry a pixel shader and a triangle topology and no mesh subobject | **wrong for 24**; 23 not seen at run time |
| **24 = console mesh shader** | sh1shu1: 430 of 430 carved sizes match mesh containers. lines9686-rgb: reading 24 as MS gives 180 s without an exception and more than 65,536 `PresentX`; as VS the title dies at 28 s | A/B on real shaders, no stubs | measured by others |
| 23 | open | static: the title writes 24 when two flag bits (positions 56 and 57) of the shader object's flags are set, and only in that branch writes 23 if a second shader object exists. That fits AS plus MS and does not rule out HS. The producers of the two byte blocks are the same function with the same mode; nothing here shows the stage. The forge's own corpus scans find no HS, DS or AS among 26,503 shaders and never use the library slots 3 to 5, which proves nothing about 23 | open; ask for a run-time dump of the 23 object's flags |

The correction of #8815 (24 becomes the mesh shader when no PC mesh subobject is present; the HS remap is dropped
and 23 stays an unknown type; the tests move into the mesh-capable part of `d3d12_mesh_test`) is **in progress**
when this note is written.

### 5.4 List slots 131 and 132, the list IID

| Item | Finding | Status |
| --- | --- | --- |
| Slot sequence | after a list is created the title calls slot 132, slot 131 and `Close` (slot 9, tested). The loop repeats it with another allocator type. The results of 132 and 131 are not tested | deduced |
| Argument values | already in [first-boot-1021.md](first-boot-1021.md). All passed by value; no proof that a value is a size, a pointer or a mode | deduced |
| #7076's handling | slots 131 and 132 are classed as not forwarded, logged, answered 0. Consistent with the missing test, enough for the creation and the `Close` to pass; it validates no effect and no rendering | measured on #7076's code (Codex) |
| IID | across all of `.rdata`, only one GUID has the shape `xxxxxxxx-4024-474c-98cf-645b5?dc4874` (the one of stop 5). The neighbouring pairs `55050859` and `8754318e` are not there (this does not exclude other sections or run-time construction). The same IID is used with allocator type 0 (first list) and type 2 (loop), so the call sites do not map it to one of the four names listed in Leonida #3520 (compute, copy and so on) | deduced; only the first 32 bits match PC `ID3D12GraphicsCommandList6`, so "the console IID is List6" stays assumed |
| Independent test | with the same device and a DIRECT allocator, `CreateCommandList` gives `E_NOINTERFACE` for the console IID and succeeds for the PC `ID3D12GraphicsCommandList6` IID. Substituting the IID and returning the PC list is not enough: the PC list has 80 slots and the title calls 132 | measured (Codex, synthetic) |
| Queue helper | the queue setup helper creates a fence (device slot 36, PC fence IID) and returns false without it; nothing there depends on the queue index or its flags | deduced |

### 5.5 `inline_scan` and the limits of a static scan

`tools/inline_scan.py` of Leonida (#7095) scans 26,674,075 instructions of the executable and classes 110 sites as
command-list writes that follow a limit check, 0 unchecked, with 2,720 stores labelled "unrelated". Our review found
that **the count is a measure of the classifier, not a proof**:

| Check | Result | Status |
| --- | --- | --- |
| guarded list writes among the "unrelated" | 5 checked by hand, all guarded: load of the cursor, compare with a limit field, branch, and a fallback call (a slot 12, 13 or another method) when the cursor reaches the limit | measured by hand (Codex) |
| second pass over the 2,720 | 10 candidates in 6 functions (a cursor load plus list-like fields within 130 instructions). 4 more looked at: same pattern | deduced |
| a false positive in the 110 | one "checked" site writes into a local structure that is not a D3D12 list | measured (Codex) |
| output labels | 12 "unrelated" sites also carry the `fields` label, so counting `fields` lines gives 24, not 12 (our comment on #7095) | measured |
| reachability | none is established; a static pattern is not an execution | stated |

So the safe reading is: the title appears to check the cursor before writing inline, and the scan undercounts.

### 5.6 The two Vulkan messages

Read in the forge code (Codex, forge head `2ac38583`), not measured on the title. Both messages come from
lines9686-rgb's run.

| Message | Meaning | Status |
| --- | --- | --- |
| "a bindless image array is full; dimension 1" | the number is the runtime's internal image dimension enum (0 plain 2D, **1 2D array including converted cubes**, 2 3D, 3 1D), so it is **not** the title's 1D textures. Sampled capacity of the 2D-array binding is 1,024 by default, entry 0 reserved for a fallback, so at most 1,023 usable. Indices below `storage_capacity` are reserved to storage views and a storage view looks only there: the array can be "full" while sampled-only indices remain. No inference about the number of descriptors the title asks for | deduced from code; the title's demand not measured |
| "storage view not supported for depth-stencil image format: 124" | 124 is `VK_FORMAT_D16_UNORM`, the runtime's Vulkan format, not a DXGI value. Only `CreateUnorderedAccessView` passes the storage flag in this revision, so a UAV creation reached a resource whose Vulkan format is D16, and the view was refused before `vkCreateImageView`. The descriptor words are still copied for a non-multisample image after the failure, so nothing is registered but the descriptor is written. The conversion table maps D16 and, for a depth resource, `R16_TYPELESS` to it; the message does not say which one | deduced; source format and caller to be logged at the UAV creation point |

## 6. Our mistakes, so as not to repeat them

| Mistake | What happened | What we do now |
| --- | --- | --- |
| Queue `Flags` | measured the `E_INVALIDARG` on slot 48 and proposed a fix in a review comment on #6935; #6954 and #7037 already covered it | check open pull requests and the coordination issue before writing a fix |
| Console list IID | opened #7124 for the slot 73 IID; #7076 already had the console list proxy. Closed in its favour, the measurements moved to #7076 | same. A plain IID remap does not give a list with 132 slots |
| Prediction taken for a stop | the first note predicted `XErrorReport` as the first stop: not reproduced in the bisect (the base already got past it) | label predictions as predictions, keep them separate from measures |
| Exit code as a stop | 127 or 3 in under a second with no event in the trace looked like an unexplained exit | an in-image `int3` is not a traced event; add a logging exception handler first (work item W12) |
| "602", then "602 to 219", then "3,140 to 219" | the first figures compared two different local stacks. The corrected one (same stack, one commit changed, 57,423 calls in both runs) was 3,140 refused to 219, created 54,282 to 57,203. But **it was measured with `LEONIDA_STUB_PIPELINES=1`**: the pipelines were accepted while their vertex program was never found. The figure does not support the VS reading | publish the stack and the options with every count; never compare runs of different stacks; measure without the stub option when the claim is about creation |
| Type 24 as VS | after reading AS and MS, then HS and VS, #8815 mapped 24 and 23 to VS and HS. sh1shu1 and lines9686-rgb showed 24 is the mesh shader | rework of #8815 in progress; 23 stays unknown until it is seen at run time |
| #6279 | still open while #1864 on main already produced the same bytes | after a change on main, re-measure and close an overlapped PR |
| Overlapping slot 55 work | #8869 also served slot 55 | we kept slot 55 in #7759 and proposed the other PR for slot 81, to be decided by the maintainers |

## 7. What this implies for AnyXboxX

Since PR #109 the proxy of TomBox is the default
(`runtime/build/tombox`), and it serves `d3d12_xs`, `xmem` and `XCRDAPI`. Everything above says what that proxy must
still serve, from the slots of Leonida's boot. "TomBox" columns come from [tombox.md](tombox.md) and have not
been run against the title.

| Need seen on the title | What TomBox has | Gap, by this note | Status |
| --- | --- | --- | --- |
| list slots 131, 132 | table of 110 slots (its `vt` has 128), so 131 and 132 are out of range | log the arguments and return `E_NOTIMPL`, never 0 (the title ignores them); needs the console list proxy | gap measured on the table; the slots' role unknown |
| list IID | the console IID is in TomBox's GUID list | adapt the IID; check it for the allocator types 0 and 2 | assumed |
| slot 55 | `create_derived_pso_x`, tags 0 to 3 | tag 5 (stencil masks) refused; #7759 of ours reads it from the title's writes | tag 5 assumed |
| slot 57 | `DeserializeGraphicsPipelineState` assigned | the title's shape (root signature, seven blocks, 0xFFFFFFFF absent) against it is not compared | to check |
| slot 86 | not assigned | stream of 0x2b0 bytes with console subobjects, type 24 as mesh shader, 23 unknown; a quiet failure leaves a null PSO | gap measured |
| slots 70, 25, 127, 133 | slot 127 out of the 90-slot device table; others not compared | shapes above; 127 needs an extension of the table | gap measured |
| device loss | none | the trap of 5.1: never return the four HRESULTs | deduced |
| `XGSuggestSwizzleMode` | none | still blocked; #6891 (Jadiac5, merged) serves both functions in Leonida | unchanged |
| audio (`AcpHal`, `chad`, `xapu`) | five `xapu` functions only | the title crashes after `ApuCreateHeap` without `AcpHal` (#6885, #6981) | measured |
| frame pacing | not compared | `PresentX` needs a frame token issued by the device-side wait (#7685) | measured on Leonida |

Order of work from this note, each item testable without the game (rule 1, rule 4):

1. a synthetic test for the HRESULT trap: the proxy's failure paths never return the four values;
2. console command lists in the proxy, with slots 131 and 132 as logged no-ops that return `E_NOTIMPL`;
3. a synthetic stream for slot 86 with types 24 as mesh shader, and for slot 55 with tag 5;
4. compare the shape of slot 57 with TomBox's `DeserializeGraphicsPipelineState`, by a unit test built from the field positions of section 5.2;
5. keep a table of the frame-loop PRs and their stack in this folder, updated at each pass, so a result is always tied to its base.

## Sources and credits

- Measurements: runs on the maintainer's machine, by Claude; static analysis by Claude (REA, capstone, targeted
  Ghidra) and by Codex (Ghidra, capstone, an independent C++ test, reading of Leonida's code). Reports and logs stay
  outside the repository.
- Leonida pull requests and issues, by author as listed in the tables: undeadhuman37, Stalkerino, labj1987, NiiRoZz,
  Kam471, mariuslozo-boop, sennecools, Pipodi, NolanCotter, Khuraie, tedin7, mrp1no, zzEmmazz, KkelebekK,
  LouisRiverstone, justr4mzi, Cattn, lines9686-rgb, rafaelpssz, Faterek, RipcuBAN, Jadiac5, tibzejoker, Spacecer2,
  alexander-src, DevilNine, Vadson159, TreXito, Krashgamer, gookie-dev. Ours: #7759 (slot 55, merged into the
  forge `main`, validated by Yoshi-OOF), #8815 (type 24, being corrected), #6279, #7124 (closed). Our measured
  comments are on #6935, #6986, #7006, #7021, #7076, #7095, #7110, #7201, #7235, #5680, #6279, #7937, #7969, #8003,
  #8023, #8033, #8043, #8055, #8869 and #8815. Coordination: #6910, #7049, #8186, #8851.
- Measurements by others: Yoshi-OOF (slot 55 derivations on forge `main`), sh1shu1 and lines9686-rgb (mesh shader
  A/B, Vulkan messages), dracbat (reported the frame token fix; the PR is #7685 by Cattn),
  KkelebekK (public stop ladder).
