> Français : [premier-demarrage-1021.md](../fr/recherche/premier-demarrage-1021.md)

# First boot of the 1.0.2.1 executable: what we measured

Continued the same evening, up to the frame loop, in [leonida-boot-2026-11-23.md](leonida-boot-2026-11-23.md).

State as of 23 November 2026. This note files the research of that day on `game_scarlett_master.exe`
(build 1.0.2.1) and on the first launches of that executable under Leonida's `gta6rt`. It supersedes the
"executable absent" state of [first-executable-leads.md](first-executable-leads.md) (22 November).

**Rules 1 and 2 of [AGENTS.md](../../AGENTS.md) apply.** The note holds only function names, vtable slots,
HRESULTs, IIDs, flag values, counts and argument shapes, gathered on a lawfully owned copy (interoperability
findings). No disassembled code, no data, no address or RVA and no file of the game is reproduced.

**Measured and assumed.** Every row says which it is. *Measured* means observed on a run or read from the
file by a reproducible method. *Assumed* means deduced, or true only on one configuration. The local reports
behind this note (static analysis by Claude with REA and capstone, reports by Codex, launch logs) stay outside
the repository.

## 1. Triage of the executable

| Item | Value | Status |
| --- | --- | --- |
| SHA-256 | `e95020b2fa4a721db3df59fdecf75621730ac559d8835252f6c3b658d5a08993` | measured |
| Size, format | 120,181,760 bytes, PE32+ AMD64, subsystem 2 (GUI), image 184,995,840 bytes, relocations present | measured |
| Entry point | MSVC startup stub, then `WinMain` | measured |
| Functions | 299,257 `.pdata` entries | measured |
| Imports | 587 named imports in 68 DLLs, 0 delay-loaded, no ordinal import except `GameInput.dll!#1` and WS2_32 | measured |
| Console-side imports | 7 DLLs: `xmem` (12), `d3d12_xs` (3), `xg_xs` (2), `XCRDAPI` (1), `chad` (5), `xapu` (5), `AcpHal` (9) = 37 | measured |
| Exports | one: `__XMemAllocationHooks__` (a data export) | measured |
| Packer | none seen: entry point disassembles as the standard MSVC stub, CRT initialiser tables are readable, `.text` entropy 6.754, and the launches below run game code up to in-image `int3` | measured; entropy alone does not prove there is no compressed region |
| Toolchain | two `.xbld` sections name two GDK editions (April 2025 Update 2 and October 2025); debug path of a Scarlett master build | measured |
| TLS | two callbacks: the first (acts on `reason == 2` only) and the second (reasons 3 and 0) | measured |
| Control Flow Guard | `GuardFlags = 0`; the three CFG pointers are initialised in the file to stubs of the image | measured |
| Package DLLs | `XboxBvhBuilder` (7 imports), `amd_fidelityfx_loader_xbox` (4), `libHttpClient.GDK` (38), `Bink2Scarlett` (23), plus the MSVC runtime copies. Leonida's `make_game.py --check` reports every expected package file present | measured; presence, not authenticity |

The provenance of the package is not authenticated, as in the other notes.

## 2. Import coverage

Three views of the same 587 imports. They are static name matches: a named export proves neither a complete
implementation nor a correct ABI.

| Provider | Result | Source |
| --- | --- | --- |
| Leonida `gta6rt --dry-run` | shim 52, host Windows 432, package module 72, **unimplemented 31** (code 0) | measured, Codex |
| Leonida, static table | the same split, with 30 unimplemented: `XMemTransferMemory` is served by the source but reported missing by an older local binary | measured, Claude |
| AnyXboxX coverage tool | package 209, AnyXboxX 6, TomBox 15, Windows 341, **missing 16** (9 `AcpHal`, 5 `chad`, `DStorageGetFactory`, `XGSuggestSwizzleMode`) | measured, Codex |

The 16 of AnyXboxX and the 31 of Leonida are not comparable: AnyXboxX counts TomBox's DLLs as available
(through `ANYXBOXX_TOMBOX_DIR`) and counts all Windows API sets as served.

Imports with no implementation in Leonida (31):

| DLL | Functions | Status |
| --- | --- | --- |
| `xmem.dll` (8) | `XMemAllocDefault`, `XMemFreeDefault`, `XMemAllocatePhysicalPages`, `XMemFreePhysicalPages`, `XMemMapPhysicalPages`, `XMemMapPhysicalPagesScatter`, `XMemIsToolsMemAvailable`, `XMemTransferMemory` | measured |
| `d3d12_xs.dll` (1) | `D3D12ConfigureVirtualMemoryX` | measured |
| `xg_xs.dll` (2) | `XGCreateTextureComputer`, `XGSuggestSwizzleMode` | measured |
| `XCRDAPI.dll` (1) | `DStorageGetFactory` | measured |
| `chad.dll` (5) | `XDspActivate`, `XDspConnectWithMaximumStreamLimit`, `XDspDeactivate`, `XDspDisconnect`, `XDspSubmitCommand` | measured |
| `xapu.dll` (5) | `XApuConnect`, `XApuDequeueResult`, `XApuDisconnect`, `XApuEnqueueCommand`, `XApuGetResultCount` | measured |
| `AcpHal.dll` (9) | `AcpHalCreate`, `AcpHalAllocateShapeContexts`, `AcpHalReleaseShapeContexts`, `ApuAlloc`, `ApuCreateHeap`, `ApuFree`, `ApuIsVirtualAddressValid`, `ApuMapApuAddress`, `ApuMapVirtualAddress` | measured |

An absent import binds to a thunk that returns 0 and writes no output pointer.

Where AnyXboxX stands for the same imports (reading `runtime/` and the TomBox crates, no run):

| Group | AnyXboxX `runtime/` | TomBox DLL (`ANYXBOXX_TOMBOX_DIR`) | Status |
| --- | --- | --- | --- |
| `xmem` | `XMemAlloc`, `XMemFree`, `XMemAllocDefault`, `XMemFreeDefault`, hooks, `XMemVirtualQuery`, `XMemGetWorkingSetStatistics` | `XMemVirtualAlloc`, physical pages, scatter, `XMemIsToolsMemAvailable`, `XMemTransferMemory` | symbols read; behaviour on the shapes of section 4 untested |
| `d3d12_xs` | four root-signature forwarders to `d3d12.dll` | `D3D12XboxCreateDevice`, `D3D12ConfigureVirtualMemoryX` | symbols read |
| `xg_xs` | none | `XGCreateTextureComputer` | `XGSuggestSwizzleMode` is nowhere |
| `XCRDAPI` | none | none | `DStorageGetFactory` is nowhere |
| `xapu` | none | five functions | |
| `chad`, `AcpHal` | none | none | |

## 3. Static start-up path

Everything below is measured by static disassembly, with direct calls only. The game calls its imports
through indirect paths (vtables, callback tables), so "reached from `WinMain`" is a lower bound.

| Step | Where | What |
| --- | --- | --- |
| 1 | entry point | `__security_init_cookie`, then `__scrt_common_main_seh` |
| 2 | C initialisers | table of 4 entries (`_initterm_e`) |
| 3 | C++ initialisers | table of 1,115 entries (1,114 non-null), almost only `VCRUNTIME140`, `malloc` and `atexit`. None calls a console import directly |
| 4 | dynamic TLS | `__dyn_tls_init`; callback 1 runs 15 `thread_local` initialisers only for `DLL_THREAD_ATTACH`, so the runtime must call it with reason 2 for each guest thread. The TLS index in the file must be overwritten; the block is 5,248 bytes |
| 5 | `WinMain` | first action: `XGameRuntimeInitialize` of the statically linked GDK library. A negative HRESULT leads to `int3` |
| 6 | GDK loader | `GetModuleFileNameW(NULL)` with the file name replaced by `xgameruntime.dll`, `GetFileAttributesW`, registry value `ForceUseLocalServices` under `GamingServices`, `GetPackagesByPackageFamily`, then `LoadLibraryExW("xgameruntime.dll", NULL, 0x800)` (`0xA00` in local mode) |
| 7 | six `GetProcAddress` | `QueryApiImpl`, `InitializeApiImpl`, `InitializeApiImplEx`, `InitializeApiImplEx2`, `XErrorReport`, `UninitializeApiImpl`. `QueryApiImpl`, `XErrorReport`, `UninitializeApiImpl` and one `Initialize*` must be non-null, else HRESULT `0x89240101` then `int3` |
| 8 | initialiser call | two 64-bit constants (`0x633600002712`, `0x65f400001edd`) and an options word with bits `2`, `4`, `8` added. Meaning of the constants: assumed |
| 9 | console test | the console-mode function reads `HKLM\Software\Microsoft\Windows NT\CurrentVersion\OEM`, DWORD `ConsoleMode`. Absent returns 1. Only converted values 1, 6, 7, 8 pass, else `int3`. Raw `0x64` converts to 7, `0x60` to 6, `0x68` to 8 |
| 10 | after the GDK | six `CreateSemaphoreExW` wrappers, `RegisterAppStateChangeNotification` and `RegisterAppConstrainedChangeNotification`, then game init and the main loop |
| 11 | graphics init | calls the device-and-window init then, later, the second memory family init |
| 12 | device-and-window init | `RegisterClassExA`, `CreateWindowExA`, `ShowWindow`, `D3D12XboxCreateDevice(NULL, &params, riid, &device)`, then a long run of device methods, then the memory front of section 4 |

`D3D12XboxCreateDevice` is called with a 0x40-byte parameter block whose first two fields are `0x20004` and
`0x100000`, and a `riid` taken from read-only data. The HRESULT is tested for sign.

Other static facts for later: `DStorageGetFactory` is used lazily by a file device .
It needs the output pointer non-null and HRESULT 0, else `int3`, then calls method at vtable offset `+0x40`
with `0x2000000` (a staging size: assumed), tested for sign. `GameInput.dll!#1`
binds to the host's `GameInput.dll`, because Leonida registers its shim under `GameInputCreate`, not `#1`.

## 4. Stops measured under `gta6rt`

Own copy of the executable, Windows 11, RTX 4070 SUPER, `gta6rt` built from Leonida `main` `1cc6e3c1`, package
mounted read-only. The first column is the order we met them. Fixes cited are open or closed pull requests of
[Leonida](https://github.com/ItsNotRexed/Leonida), credited to their authors. "Local stand-in" means a private
patch, never published, used to find the next stop.

| # | Stop | Cause | Status | Pull request that treats it |
| --- | --- | --- | --- | --- |
| 0 | exit code 127 or 3 in under a second, no import called, 12 trace events (maps, `DllMain`, TLS callbacks), no exception traced | an `int3` is not an event in the trace; explained by the later stops | measured; explanation assumed | none: lesson for tooling (section 5) |
| 1 | `UNIMPLEMENTED d3d12_xs.dll!D3D12ConfigureVirtualMemoryX`, exit 3, called from the memory init of the title | import absent. Same stop on the reference build, 3 runs in `stop` mode and 3 in `log` mode | measured | #6792 by undeadhuman37 (merged 2026-11-23) |
| 2 | `int3` `0x80000003` in the same init, right after `Configure...` returned `S_OK` | third `XMemVirtualAlloc` of the init (size `0x49400000`, attributes `0x20003000`, `r9 = 0x183`, protection `0x22004`) fails with `ERROR_NOT_SUPPORTED` | measured with a local stand-in that ignores the attribute word and keeps `protect & 0xff` | #6709 by Stalkerino (64 KiB reservation forms), #4531 by labj1987 (2 and 4 MiB pages), #6878 by mrp1no (closed; non-zero attribute word, console protection bits), #6919 by NiiRoZz (open; 2 MiB pages and placeholders). Added in turn on top of #6792, #6709, #4531 and #5253 (maxxgabriel) left the same `int3` (bisect) |
| 3 | `int3` in the physical-memory allocation wrapper after `XMemAllocatePhysicalPages(rcx = 0x280, ...)` | physical pages not served. A stand-in returning `rcx * 64 KiB` of `VirtualAlloc` gives the same stop; `XMemMapPhysicalPages` is not called before it. The real handle shape is not established | measured; cause assumed | #6919 (the seven missing `xmem` imports); the coordination issue lists #6878 as covering the same ground |
| 4 | `int3` in the device init: device slot 48 (`CreateCommandQueueX`), second and third queue | queue descriptions `{Type 2, Priority 0, Flags 0x1 / 0x2 / 0x3, NodeMask 0}`. PC accepts only `0x1`, so `0x2` gives `E_INVALIDARG` | measured | routing in #6935 by Kam471 (merged); the `Flags` measurement is our review comment on it; #6954 by Pipodi and #7037 by sneylol (open) |
| 5 | `int3` in the device init: device slot 73 (`CreateCommandListX`) | `riid = c3827890-4024-474c-98cf-645b51dc4874`, the only GUID of that form in `.rdata`. The PC device answers `E_NOINTERFACE` (`0x80004002`). The same device and DIRECT allocator create the list for the PC IID `c3827890-e548-4cfa-96cf-5689a9370f80` (independent synthetic test, Codex) | measured. That the console IID is the PC `ID3D12GraphicsCommandList6` is assumed: only the first 32 bits match | #7076 by NiiRoZz (open; console list proxy, depends on #6935 and #2490 by NolanCotter). Our #7124 was closed in its favour, measurements added there |
| 6 | `int3` in the device init: list slot 9 (`+0x48`, `Close` in the PC table) returns a negative HRESULT | right after creation the title calls list slots 132 (`+0x420`) and 131 (`+0x418`), results unused, then slot 9, tested. The PC list has 80 slots, so a raw PC list cannot serve `+0x420`. Meaning of 131 and 132: unknown | measured; nature of the slots not established | none yet; #2490 (console-ABI command lists) is the nearest |

Argument shapes of list slots 132 and 131 (static, Codex). First list: slot 132 `(0x700000, 0, 0, 0xffffffffff00)`,
slot 131 `(0xc3c0, 0x80020000, 0, 0xffffffffff00)`. Second list, in a loop: slot 132 `(0x100000, 0, 0, 0xffffffffff00)`,
slot 131 `(0xc380, 0x20000, 0, 0xffffffffff00)`. The constants are passed by value; nothing says they are sizes or
pointers.

### Predicted by static analysis, not met as a stop on our side

| Prediction | Detail | Status |
| --- | --- | --- |
| `XErrorReport` not exported by the shim | `GetProcAddress("XErrorReport")` returning NULL leads to `0x89240101` then `int3` in `WinMain`. At the time of the bisect, #6582 (Kam471, open) had no effect on the stop, so the base already got past it | assumed; the prediction was not reproduced |
| `DStorageGetFactory` | an unimplemented thunk leaves the output unset and the title hits `int3` | measured statically; run-time behaviour reported by others (stop 24 below) |
| `GameInput.dll!#1` | binds to the host DLL, bypassing the SDL3 shim | measured statically |

### Public measurements by others

KkelebekK (Linux, Wine 11.21, Leonida #6910, 2026-11-23) went further with local stand-ins and numbered their own
stops 13 to 26. Their numbering is not ours. We only list what bears on AnyXboxX.

| Their stop | Shape | Relevance |
| --- | --- | --- |
| 13 | descriptor heap: console heap description carries the title's address at `+0x10` and a size at `+0x18`; heap slot 9 must return that address (#6915) | heap model |
| 16 | device slot 127, the `CreateCommittedResource3` shape, with a barrier layout | device ABI |
| 19, 20, 21 | device slots 41, 61, 74 and 86 | device ABI |
| 22 | `XGSuggestSwizzleMode`, then `XGCreateTextureComputer`; the computer's slot 8 is called once per mip. An `E_NOTIMPL` that leaves the output unset makes the title call through garbage (#6891, #7009) | `xg_xs` |
| 23 | `XMemMapPhysicalPagesScatter`, one call of `0x1a40` pages | `xmem` |
| 24 | DirectStorage opens its queue; #4348 by mariuslozo-boop answers zlib, BCPack and swizzle requests with `E_NOTIMPL` and the run goes on | `XCRDAPI` |
| 26 | device slot 57, once per pipeline, then slot 108 | pipelines |

Static analysis by Codex on slot 57 : `rdx` points at a block
whose first 8 bytes are an object reference, `riid` is the PC `ID3D12PipelineState`
(`765a30f3-f624-4c6f-a828-ace948622445`), the output pointer is the fourth argument, and the result is tested for sign.
The first field is very likely a root signature: it comes from device slot 16 (`+0x80`, `CreateRootSignature`
shape) with the PC `ID3D12RootSignature` IID (`c54a6b66-72df-4ee8-8be5-a946a1429214`). The structure after the
first field is not reconstructed. The fence of the queue setup comes from device slot 36 (`+0x120`) with the PC
`ID3D12Fence` IID (`0a753dcf-c4d8-4b91-adf6-be5a60d95a76`); a failing creation makes the helper return false.

### Memory front, in call order (static)

Inside the device-and-window init, after `D3D12XboxCreateDevice`. Arguments are shapes only.

| Call | Shape | Failure branch |
| --- | --- | --- |
| device slot 49 (`+0x188`) | `(0x11f, 1)` | result unused |
| `XMemVirtualAlloc` | `(0, 0x10000, 0x20401000, 0x283, prot 0x42404)` | NULL: fallback, no `int3` |
| device slots 26, 8, 48, 9, 73, 79, ... | queues, allocators, first list (probable roles, from the PC table) | results untested, except slots 48 and 73 (stops 4 to 6) |
| `XMemVirtualAlloc` x6 | first `(…, 0x42000, 7, 1)`, then five with `0x20406000`, `r9 = 0x27`, protections `0x1004`, `0x1404`, `0x8404`, `0x2404`, `0x2004` | NULL: cleanup branch, no `int3` |
| `XMemAllocatePhysicalPages` | `(0x280, &count in/out, pfn array of count * 8 bytes)`, returns BOOL | 0: cleanup |
| `XMemMapPhysicalPages` | `(reserved address, count, pfn array)`, returns a pointer | NULL: cleanup |

The second family (later) asks `XMemVirtualAlloc(0, size, 0x20402000, 0x21, 4)` then
`(…, 3, 0x801)`, whose NULL leads to `int3`; then `XMemAllocatePhysicalPages(0x380, ...)`, a mapped block,
device slot 74 (`+0x250`) and device slot 8 (`+0x40`), each tested for sign with `int3` on failure. For the
attribute word `0x20402000`, a NULL return must never happen.

## 5. What this means for AnyXboxX

Order of work, by the measured stops. "Assumed" items need a test before anyone relies on them.

| # | Work | Measured basis | Status of the basis |
| --- | --- | --- | --- |
| W1 | **Done, PR #107.** **`xmem` behaviour on the real shapes.** Add a synthetic test (rule 4, no game file) that calls the `xmem` AnyXboxX loads, ours or TomBox's, with the measured `XMemVirtualAlloc` words (attributes `0x20401000`, `0x20402000`, `0x20406000`, `0x20003000`, `0x42000`; `r9` in `0x3`, `0x7`, `0x21`, `0x23`, `0x27`, `0x183`, `0x283`; protections listed above) and with the physical-page triple (`0x280`/`0x380`, count in/out, 8-byte PFN array, mapped pointer). It must never return NULL for `0x20402000` | stops 2, 3; front call table | measured shapes; behaviour of TomBox's DLL unknown |
| W2 | **Done, PR #107.** `runtime/xmem` itself serves none of the seven missing imports (`XMemVirtualAlloc`, three physical-page calls, scatter, `XMemIsToolsMemAvailable`, `XMemTransferMemory`). Decide: keep relying on TomBox, or implement them in C++ with the ERA decoder, and say so in [tombox.md](tombox.md) | section 2 | measured |
| W3 | **Done, PR #107.** `XMemAlloc(0x60000, 0xf0814000)`: check that the ERA decoder of `runtime/xmem/era_heap.cpp` does not refuse this word ("physical memory refused" returns NULL, which the title treats as an error branch) | static call site | decode of this word assumed |
| W4 | **Done, PR #107.** `D3D12ConfigureVirtualMemoryX`: the AnyXboxX `d3d12_xs` does not export it (TomBox does, and rejects a NULL description with `E_INVALIDARG`). Test a non-null description whose second 32-bit field derives from a size, expect `S_OK` | stop 1 | measured shape; field meaning assumed |
| W5 | **Resolved by reusing TomBox (section 6).** **Device slot table.** Compare the slots the title uses before the first `Close` failure (8, 9, 16, 26, 36, 41, 48, 49, 57, 61, 73, 74, 79, 86, 127, and list slots 9, 131, 132) with TomBox's `d3d12_xs` (194 of 200 methods claimed). Output: a table slot, shape, covered or not, in [tombox.md](tombox.md) | sections 3 and 4 | slots measured; TomBox coverage not compared |
| W6 | **Not done: needs the console list proxy first (Leonida #7076), see section 6.** Console command list: find out what list slots 131 and 132 are, from TomBox's console list ABI, before anyone writes a wrapper. Do not pass a raw PC list to the title | stops 5, 6 | slot meaning unknown |
| W7 | **Done, this PR: minimal `XCRDAPI.dll`.** **`DStorageGetFactory`** is missing from AnyXboxX and TomBox. A minimal `XCRDAPI.dll` must write the output pointer, return 0, and expose the method at `+0x40` returning a non-negative HRESULT; file reads can answer `E_NOTIMPL` as #4348 does. Leonida's #4348 is GPL-3.0, reusable with credit (rule 2) | static, stop 24 | measured |
| W8 | **Blocked, see section 6.** **`xg_xs`**: `XGSuggestSwizzleMode` is missing. `XGCreateTextureComputer` must hand back an object whose slot 8 exists even if it writes no tiled data, and no stub may leave an output pointer unset. `anyxboxx-texture` (SW_4K_S) is the natural back end | stop 22 (KkelebekK) | measured by others |
| W9 | **Done, this PR: `tests/test_gdk_boot.py`; return type kept, see section 7.** `xgameruntime`: `XErrorReport` is exported with a `void` return (`runtime/xgameruntime/xgameruntime.def`); the title only tests the pointer, so this looks harmless. Add a launcher test that the six `GetProcAddress` names resolve, that the options bits `2`, `4`, `8` are accepted, and that `xgameruntime.dll` loads by full path when the game executable sits in another folder | steps 6 to 8 | harmless return type assumed |
| W10 | **Done, this PR: see section 7.** `ConsoleMode`: `runtime/xgameruntime/registry.cpp` serves `0x64`, which converts to 7 and passes the title's test. Keep a test pinning `0x64`, and the `RegOpenKeyW` variant (the title uses it) | step 9 | measured and consistent |
| W11 | **Done, this PR: new threads verified; threads older than the install get no reason 2, as under Windows, see section 7.** **TLS**: check that `anyxboxx_tls.dll` calls the title's callback 1 with reason 2 for every thread, existing or new, and that the index in the file is overwritten. The block (5,248 bytes) fits the 256 KiB capacity | step 4 | measured size; thread coverage not tested |
| W12 | **Observability.** The native launcher has no handler for `0x80000003`: a title `int3` ends in a bare exit code (stop 0). Add a vectored handler that logs the address of the first `int3` or exception and the module, so a bisect gives a stop instead of an exit code | stop 0 | measured |
| W13 | **Done, this PR: host dependency documented in [day-one.md](../day-one.md), no shim, see section 7.** `GameInput.dll!#1`: AnyXboxX has no GameInput shim. The title's single caller binds to the host DLL. Choose: serve ordinal 1 from the launcher, or document the host dependency | static | measured; ABI version unchecked |
| W14 | Audio (`chad` 5, `xapu` 5 from TomBox, `AcpHal` 9): missing, but not established as boot-blocking, since the call chain is indirect. Keep after W1 to W8 | section 2 | assumed |
| W15 | **Done, this PR: recorded in [day-one.md](../day-one.md), section 3.** Host-only imports: `api-ms-win-core-psm-appnotify-*`, `ext-ms-win-rtcore-ntuser-*`, MFPlat, WS2_32 by ordinal. Native mode depends on a recent Windows host; record this in [day-one.md](../day-one.md) | section 2 | measured on Windows 11 only |
| W16 | **Not done: no public source, see section 7.** `__XMemAllocationHooks__`: the title exports a data symbol that no source of Leonida reads. If `XMemAlloc*` misbehaves, check whether the console runtime reads it through `GetProcAddress` on the executable | export table | purpose assumed |

W1, W3, W7, W8, W9, W10, W11 and W12 can be done from public documentation and synthetic tests. W5 and W6
need TomBox's sources only. None of them needs to read the game again.

## 6. Results of W5 to W8 (this PR)

- **W5.** The device slot table is in [tombox.md](tombox.md), frozen by `tests/test_d3d12_device_slots.py`.
  AnyXboxX served no device slot. **Resolved by reusing TomBox**, not by writing a proxy: `tools/build_tombox.py`
  compiles TomBox's `d3d12_xs` (64 exports, device proxy to the host D3D12), which `runtime/build/tombox` now serves
  by default. `tests/test_tombox_native.py` gets a device from `D3D12XboxCreateDevice` (IID_ID3D12Device, WARP or GPU).
  The same run checks W1 (`XMemVirtualAlloc` with `0x20402000` never returns NULL) and the W5 `DStorageGetFactory`
  output, both through TomBox's `xmem` and `XCRDAPI`. Not in TomBox and still ours: `xgameruntime`, `kernelx`,
  `anyxboxx_tls`; and the xmem export `XMemGetAllocationHooks`, which TomBox lacks (no import of it was found in the
  title's static analysis; to propose upstream).
- **W6.** Not done. Slots 131 and 132 belong to a console command list, which AnyXboxX does not have; the list
  proxy (Leonida #7076) comes first. When it exists, these slots must log their arguments and return `E_NOTIMPL`
  (the title ignores their results), never 0.
- **W7.** `runtime/xcrdapi/` builds `XCRDAPI.dll`. `DStorageGetFactory` writes a factory with the console layout
  (slots 0 to 8). Slot 8 (`SetStagingBufferSize`, +0x40) returns `S_OK` and logs the size, which is ignored because no
  staging buffer exists yet: a documented no-op that keeps the title's sign test passing. `CreateQueue`, `OpenFile`,
  `CreateStatusArray` and `SetCpuAffinity` return `E_NOTIMPL` with a null output. File reads are not served yet.
  Tests: `tests/test_xcrdapi.py`.
- **W8.** Blocked. `XGSuggestSwizzleMode` has no public signature (searched in Leonida, TomBox, the local GDK and
  public documentation), so no output argument can be written safely. TomBox's `XGCreateTextureComputer`
  (`crates/compat/xg_xs/src/lib.rs`, lines 372 to 381) returns early for a dimension above 4 and on its error branch
  without writing `*out`. Observed in the source, not measured on the title. The fix belongs upstream, since
  `third_party/` is not edited here.

## 7. Results of W9, W10, W11, W13 and W15 (this PR; W12 is handled in issue #113)

- **W9.** `tests/test_gdk_boot.py` builds a title (with `tests/pe_builder.py`) in its own folder, away from the
  runtime and without an `xgameruntime.dll` beside it, and replays steps 6 to 8: `GetModuleFileNameW(NULL)` with the
  file name replaced by `xgameruntime.dll`, `LoadLibraryExW` of that full path with `0xA00`, then of the bare name with
  `0x800`, the six `GetProcAddress`, and the three `Initialize*` with the two constants and options `2 | 4 | 8`. Both
  loads must return the same AnyXboxX module, the six names must resolve, and each initialiser must return `S_OK`.
  The same program runs under `anyxboxx.exe` and under the Python native mode. Without the full-path redirection the
  first load fails and the result drops to 0 (checked by mutating the Python shim). The argument order of the
  initialisers (constants in `rcx`, `rdx`, options in `r8`) is assumed; our initialisers ignore their arguments when
  the Windows runtime is off, so the test pins that no option bit is refused.
  `XErrorReport` keeps its `void` return. Microsoft Learn documents no `XErrorReport`: the
  [XError members](https://learn.microsoft.com/en-us/gaming/gdk/docs/reference/system/xerror/xerror_members) page
  lists only `XErrorCallback`, `XErrorSetCallback`, `XErrorSetOptions` and `XErrorOptions`. It is an internal export
  that `xgameruntime.lib` resolves by name, so there is no public signature to fix it to; the title only tests the
  pointer (static, section 3). Its `(HRESULT, const char*)` parameters follow the first two of the documented
  [XErrorCallback](https://learn.microsoft.com/en-us/gaming/gdk/docs/reference/system/xerror/functions/xerrorcallback).
  The decision is written next to the function in `runtime/xgameruntime/xgameruntime.cpp`.
- **W10.** `0x64` is pinned twice. `tests/test_runtime_cpp.py` opens the OEM key through `AnyXboxX_RegOpenKeyW`
  (with and without a leading backslash, any case) and reads `ConsoleMode` as `REG_DWORD` `0x64`, with
  `ERROR_MORE_DATA` and the size 4 for a 2-byte buffer; `XSystemGetDeviceType` stays 7. `tests/test_gdk_boot.py`
  builds a title that imports `ADVAPI32!RegOpenKeyW`, `RegQueryValueExW` and `RegCloseKey`, as the title does, and
  reads `0x64` under `anyxboxx.exe` and under the Python native mode. Without the `RegOpenKeyW` redirection the same
  program reads 0 on a PC (checked by mutating the Python redirect list). The conversion of `0x64` to 7 happens in
  the title's `xgameruntime.lib` and is not testable here (static reading, section 3).
- **W11.** `tests/test_native_tls_callbacks.py` loads a title whose TLS callback counts each reason, keeps its first
  argument and, for reason 2 only, writes a marker into the calling thread's TLS copy, as the MSVC `__dyn_tls_init`
  does with its initialisers. Verified: the `_tls_index` of the image is overwritten (no longer `0xFFFFFFFF`); the
  callback gets the image base and `DLL_PROCESS_ATTACH` once, at install; every new thread runs it with reason 2 on
  that thread, after its area is filled and with the right index (each of four threads reads the marker), and with
  reason 3 when it exits. With the reason 2 call of `anyxboxx_tls.dll` commented out, the new-thread test fails
  (mutation of the C++ runtime, rebuilt). No C++ change was needed.
  Two threads do not get reason 2 from AnyXboxX, by design. The thread that installs the image gets
  `DLL_PROCESS_ATTACH`, as Windows gives an executable's primary thread; the MSVC CRT start-up of the title then calls
  its own dynamic TLS callback with `DLL_THREAD_ATTACH` for that thread (`exe_common.inl` and `tlsdyn.cpp` in the
  CRT sources shipped with Visual Studio). A thread born before the install gets the template copied into its area
  but no callback: Windows does the same for a DLL loaded after its threads started
  ([DllMain](https://learn.microsoft.com/en-us/windows/win32/dlls/dllmain), `DLL_THREAD_ATTACH`), and code built
  with [`/Zc:tlsGuards`](https://learn.microsoft.com/en-us/cpp/build/reference/zc-tlsguards) (on by default since
  Visual Studio 2019 16.5) initialises its `thread_local` variables on first use on such threads, because the
  copied template leaves the guard false. Running code on another thread to deliver reason 2 would need thread
  hijacking, which we do not do. That the title is built with TLS guards is assumed (its toolchain is recent, section 1,
  but the guard was not looked for). The launcher installs the TLS before the title's entry point, on the same thread,
  so in `anyxboxx.exe` the threads older than the install are only those of the launcher and of the Windows loader.
- **W13.** Ordinal 1 is `GameInputCreate(IGameInput**)`. Sources: Microsoft Learn says that in GameInput v0
  `GameInputCreate` is exported by `GameInput.lib` and always creates the v0 interface, that libraries from earlier
  releases "such as the one in the GDK" keep calling it and are forwarded to the installed runtime, and that every
  runtime supports v0 ([GameInputCreate](https://learn.microsoft.com/en-us/gaming/gdk/docs/reference/input/gameinput/functions/gameinputcreate),
  [GameInputInitialize](https://learn.microsoft.com/en-us/gaming/gdk/docs/reference/input/gameinput/functions/gameinputinitialize),
  [API versions](https://learn.microsoft.com/en-us/gaming/gdk/docs/features/common/input/overviews/input-version-interop)).
  The newer NuGet library exports `GameInputInitialize` instead, so an import of `GameInputCreate` by ordinal fits the
  GDK's v0 library. Measured on our Windows 11 host: `System32\GameInput.dll` (0.2309.26100.9818) exports
  `GameInputCreate` at ordinal 1, and so does `System32\GameInputRedist.dll` (4.0.306.0, with `GameInputInitialize`
  at 2). The ABI question of the row is therefore settled for the factory: v0, which any host runtime serves. That the
  console's `GameInput.dll` also numbers `GameInputCreate` 1 is assumed from the static import and the GDK library.
  **Choice: no shim, the host serves it.** A runtime `GameInputCreate` answering `E_NOTIMPL` (rule 3) would take
  real controllers away from a title that the host already serves, and a working one would duplicate the host
  runtime. `tests/test_launcher.py` checks that `anyxboxx.exe` binds `GameInput.dll!#1` to the host and that the call
  returns `S_OK` with an object (skipped on a host without GameInput), and that a host library missing an ordinal
  import stops the launcher with the import named (`dll!#1`), never a silent thunk. `tests/test_native_services.py`
  already checked the same binding in the Python native mode. [day-one.md](../day-one.md) says what to install when
  GameInput is missing.
- **W15.** [day-one.md](../day-one.md) now lists, under "What the Windows host must provide", GameInput (W13) and
  the host-only imports of section 2, with what is known: measured on Windows 11 only; API set availability is
  decided per device whatever the `api-` or `ext-` prefix
  ([Windows API sets](https://learn.microsoft.com/en-us/windows/win32/apiindex/windows-apisets)); `RegisterAppStateChangeNotification`
  is hosted by `twinapi.core.dll` behind `api-ms-win-core-psm-appnotify-l1-1-0`/`-l1-1-1`
  ([Microsoft Learn](https://learn.microsoft.com/en-us/windows/win32/api/appnotify/nf-appnotify-registerappstatechangenotification));
  N editions lack Media Foundation without the Media Feature Pack. Documentation only, no test.
- **W16.** Not done. The `xmem` reference pages of the GDK (`XMemSetAllocationHooks` and the related macro) are
  served by Microsoft Learn only to NDA developer programs (checked on 2026-11-24), so they are not a public source
  under rule 2, and no public page or open-source code we found reads this export. The row stays open until a public
  source or a measurement on the title exists.

## Sources and credits

- Static analysis of the executable: Claude (REA 6.10.1, capstone, `.pdata` bounds). Reports, IID scan, targeted
  Ghidra decompilations and the independent `CreateCommandList` test: Codex.
- Leonida pull requests and coordination: [#6935](https://github.com/ItsNotRexed/Leonida/pull/6935) (Kam471),
  [#7076](https://github.com/ItsNotRexed/Leonida/pull/7076) (NiiRoZz), #7124 (ours, closed),
  [#6910](https://github.com/ItsNotRexed/Leonida/issues/6910) and
  [#7049](https://github.com/ItsNotRexed/Leonida/issues/7049) (coordination, KoertLichtendonk),
  #6792, #6709, #4531, #6878, #6919, #5253, #6582, #6954, #7037, #4348, #2490.
- Our comments with the measurements: the review comment of 2026-11-23 on #6935 (queue `Flags`) and the comment
  on #7076 (command-list IID and list slots), both limited to counts, HRESULTs and the IID.
- Neighbouring projects in general: [neighbors.md](neighbors.md); Leonida's GTA 6 notes:
  [leonida-gta6.md](leonida-gta6.md).
