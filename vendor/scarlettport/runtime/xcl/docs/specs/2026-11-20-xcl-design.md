# XCL — Xbox Series X|S → Windows compatibility layer (design)

Date: 2026-11-20 · Status: awaiting approval · Working name: **XCL** (subject to change)

## 1. Goal and why

Run games built for Xbox Series X|S (GDK, "Scarlett") on an ordinary Windows 11
(x64) PC **without CPU emulation** — the game's code is native x64, so it runs
directly. The layer substitutes its own implementations of libraries that the
PC does not have (`kernelx`, XGameRuntime, D3D12.X, XAudio2 with XMA2, GameInput),
much like Wine/Proton do for Windows on Linux, and WinDurango does for
Xbox One.

**First success (in the user's words):** "getting the game running properly, i.e.
handling the files (rpf etc.) and firing up the game on the PC". In practice:
the game exe loads, all of its imports are bound, the game gets through
initialization and reads its data files (archives such as RPF are read by the
game's own code — the layer only has to provide working file operations).

The layer is meant to be **general** — for any Series title, not tailored to one game.

## 2. Goals, non-goals, anti-goals

**Goals**
- A universal loader for Series games (x64 PE) with automatic stubs for
  unimplemented functions and a full report of what is missing.
- `kernelx` + a basic XGameRuntime sufficient for the game to start,
  read files and write data (saves, cache) outside the game folder.
- A development path to graphics (D3D12.X → D3D12), audio (XAudio2/XMA2) and
  input (GameInput) — later stages.

**Non-goals (out of scope)**
- Decrypting packages (XVC/MSIXVC) and any DRM circumvention.
  The layer assumes the game files are already ordinary PE/data files.
- Xbox Live online services (XSAPI, multiplayer, achievements) — offline mode only.
- Xbox One games (XDK/ERA, D3D11.X) — WinDurango does that.
- Translating the console's native GPU code (RDNA2 ISA) if the game does not contain
  DXIL (see risks).
- macOS and Linux. The target platform is Windows x64 only. The only
  "future-proofing" requirement: the graphics backend can be swapped for vkd3d-proton (§5.7).
  Running `xcl.exe` under Wine is **not** required, so the hooking techniques
  do not have to be compatible with it. macOS on Apple Silicon would require CPU
  translation (Rosetta), which conflicts with the "no CPU emulation" premise.

**Anti-goals (failure, even if it "works")**
- **Any modification or deletion of a file in the game folder.** The game folder is
  strictly read-only; if the game wants to change something, we copy that single file
  into the overlay and change the copy. (User requirement.)
- Silent failures — every call to an unimplemented function must be logged.
- Code or headers from SDK leaks in the repository. We write the declarations ourselves
  based on public documentation (docs.xbox.com, learn.microsoft.com).
- Faking store licenses (XStore returns "offline/unavailable", not "purchased").

## 3. Constraints

- Host: Windows 11 x64. Tools available on the machine: Visual Studio 2022
  Community (MSVC, CMake and Ninja bundled with VS), Git, Python 3.12, MSYS2 gcc.
- On the Series console **only D3D12.X** is available (D3D11.X does not exist) —
  source: public GDK documentation.
- Full documentation of some topics (e.g. the Xbox shader compiler definitions)
  is under NDA; we rely on public pages only.
- No legal Series test binaries → we test on our own test
  program (§7), which has an import table like a console title.
- **Real titles are run by the user** on their own files and on their own
  computer. The implementer has no access to them and does not test on them. Work
  loop: the user runs `xcl inspect` / `xcl run` and hands over
  `report.json` + the log, and the implementer adds further functions based on that.
  The test program checks the mechanics (loader, TLS, VFS…), but it will not catch
  wrong signatures, ordinals or struct layouts, because both sides of the test
  are based on the same declarations. Only reports from
  real titles will catch those.
- Precondition for a real title: the files are already ordinary
  PE/data files, not an encrypted XVC/MSIXVC package. `xcl inspect` detects and
  reports a file that is not a valid x64 PE.
- The import names of real Series games are not known in advance (they may be
  `kernelx.dll`, `api-ms-win-*` API sets, console-only exports, imports
  by ordinal only). That is why the resolver is data-driven (aliases + an
  ordinal/stub database), not hardcoded lists.

## 4. Approaches

| | Description | Pros | Cons |
|---|---|---|---|
| A | **DLL replacement** (like WinDurango): Windows loads the game exe, and our `kernelx.dll` etc. sit next to it. | Simplest; TLS, exceptions and loading are done by Windows. | One missing function = the game does not start ("entry point not found"). Every function has to be added and rebuilt. The DLLs have to sit next to the exe → copying the exe to a temporary folder. |
| **B** | **Own PE loader** (`xcl.exe`): we map the game exe into memory ourselves and resolve imports through a central table; missing functions → automatic logging stubs. | The game always gets as far as it can; a full report of what is missing after the 1st run; nothing has to be copied into the game folder; a single point of control (paths, memory, threads). | We do ourselves what Windows normally does: static TLS, exception registration, visibility of the image to the system. |
| C | Hybrid: a copy of the exe with a rewritten import table, loaded by Windows. | Windows does TLS. | The drawbacks of A (rigid export lists), with no advantage over B. |

**Choice: B** (designer's decision; the user delegated the "how" — "make it general").
Rationale: the goal is a general layer and "the game starts"; B is the only option that gives
a start despite gaps, plus a report of what comes next, for every title. The risks of B (TLS,
C++ exceptions) are checked as the first tasks of stage M1 on the test program.

**DXVK/DXMT** (user's suggestion): considered, **not used for Series
games** — both translate only D3D8–11 (DXVK → Vulkan, DXMT → Metal/macOS),
while Series games use D3D12.X exclusively. The counterpart for D3D12 is
native D3D12 on Windows or vkd3d-proton (D3D12 → Vulkan) — see §5.7.

## 5. Architecture

A single process: `xcl.exe` (launcher + loader + all HLE modules as
static libraries). The game is mapped into this process and executed natively.

```
xcl.exe
 ├─ launcher   CLI: `xcl run <folder|exe>`, `xcl inspect <exe>`
 ├─ loader     PE mapping, relocations, TLS, exceptions, module table
 ├─ resolver   import policy, aliases, stub generator
 ├─ vfs        mounts, path translation, copy-on-write overlay
 ├─ hle/kernelx        (M1)
 ├─ hle/xgameruntime   (M1, offline subset)
 ├─ hle/d3d12x         (M2)
 ├─ hle/xaudio2        (M3)
 ├─ hle/gameinput      (M4)
 └─ common     log, configuration, report
```

### 5.1 PE loader (M1)

1. Read the game exe (read-only), check: PE32+, `IMAGE_FILE_MACHINE_AMD64`.
2. Reserve memory at the preferred `ImageBase`; if it is taken and there are relocations
   → relocate; if it is taken and there are no relocations → a clear error. (`xcl.exe` is
   built with ASLR, so the typical address `0x140000000` is usually free.)
3. Map the sections, apply relocations, resolve **regular and delayed
   (delay-load)** imports through the resolver (§5.2), set the sections' page protection.
4. **Exceptions:** register `.pdata` (`RtlAddFunctionTable`). In addition, the image
   has to be "visible" to the system: C++ exceptions on x64 use
   `RtlPcToFileHeader`, which looks the module up in the Windows loader list —
   a manually mapped image does not appear there. Solution: hook
   `RtlPcToFileHeader` and `GetModuleHandleExW(FROM_ADDRESS)` (Microsoft
   Detours, MIT) or an entry in the PEB module lists — the implementer's choice;
   test: a C++ exception thrown and caught in the game's code must work.
5. **Static TLS** (`__declspec(thread)`, widely used): allocate an
   index, write it into the game's `AddressOfIndex`, build a block for every thread
   from the game's TLS template and insert it into the `ThreadLocalStoragePointer` array in the TEB
   (extending it). New threads: through `xcl.exe`'s own TLS callback
   (it receives `DLL_THREAD_ATTACH` for every new thread, including ones from the system
   thread pool). Threads that existed before loading: fill them in at
   load time. Call the game's TLS callbacks (process/thread attach/detach).
   Exact technique — up to the implementer; multithreaded tests are required.
6. Set `PEB->ImageBaseAddress` to the game image before start (resources,
   `GetModuleHandle(NULL)` in system code) — after verifying that it does not break
   the launcher; otherwise handle it in shims.
7. Call the game's entry point on a thread with a large stack (size from the game's
   PE header, `SizeOfStackReserve`).

DLLs shipped **by the game** (middleware etc.) are loaded by the same
loader, recursively, with imports through the resolver.

### 5.2 Import resolver (M1)

For every import `dll!function` (or `dll!#ordinal`), in order:

1. **Aliases from the configuration** (e.g. `api-ms-win-core-*` → `kernelx` or → system).
2. **HLE module** with that name (`kernelx`, `xgameruntime`, …): if the function is
   implemented → our implementation. A module may have a **fallback to the
   system DLL** (e.g. `kernelx` → `kernelbase`/`kernel32` for functions with
   the same names that we do not override).
3. **DLL from the game folder** → our loader (5.1).
4. **Allowlisted Windows system DLL** (e.g. `ucrtbase`, `vcruntime140`,
   `msvcp140`, `user32`, `ws2_32`) → `LoadLibrary` + `GetProcAddress`.
5. **None** → **automatic stub**: a small x64 thunk (e.g. `mov r10, info;
   jmp handler` — without touching the argument registers) which on the first
   call logs `[STUB] dll!function` with the caller's address (RVA in the
   game image) and returns a value from the stub database (`0` by default; for functions
   known to return HRESULT — `E_NOTIMPL`; overridable in the configuration).

**Imports by ordinal:** ordinals cannot be determined from public documentation.
The resolver has a `dll → {ordinal → name, return value}` database in a data file
(e.g. TOML/JSON in the repo), extended based on reports. An unknown ordinal
gets a `dll!#N` stub, and `inspect` and `report.json` list it separately.

Runtime: `LoadLibrary*`/`GetModuleHandle*`/`GetProcAddress` called by the game
go to shims that use the same module table (HLE modules have
pseudo-handles).

### 5.3 kernelx (M1)

Function groups (signatures from public documentation + import reports;
WinDurango `kernelx.cpp`, LGPL-3.0, as a reference for the logic):

- **Memory:** `VirtualAlloc*/Free/Protect/Query` with console flags
  stripped (e.g. graphics/title memory), the `XMemAlloc/XMemFree/
  XMemSetAllocationHooks` family, physical page APIs (emulated via
  `VirtualAlloc`/file mapping), `TitleMemoryStatus` (budget from the configuration).
- **Threads:** thread names, affinity masks (console cores → PC cores),
  the `XThread*` family.
- **Files:** through the VFS (§5.4).
- **Modules:** the shims from §5.2.
- **System:** console type (`XSystemGetDeviceType` etc. — configurable as
  Series X / Series S), OS version, processor info.
- The rest (synchronization, time, …) → fallback to the system.

### 5.4 File system (VFS) (M1)

- **Mounts:** game root directory = the original game folder, **read-
  only**. Drive letters / console paths → mapping from the configuration.
  Every unknown console path is logged as `[VFS] unmapped`, so that
  a mapping can be added.
- **Writable data** in `%LOCALAPPDATA%\XCL\<title>\`: `overlay\`, `saves\`,
  `pls\` (persistent local storage), `temp\`, `logs\`.
- **Copy-on-write:** opening a game file for writing → copy **that one file**
  to `overlay\` and open the copy. Reads: overlay first, then the original.
  Deletion → a marker in the overlay (original untouched). Directory enumeration
  merges both layers.
- **Coverage:** the VFS must also catch operations from DLLs loaded by Windows
  (e.g. the CRT: `fopen` → `ucrtbase` → `CreateFileW`), so it works as global
  in-process hooks (Detours) on the file APIs (Win32 or `Nt*` — the implementer's
  choice), not only on the game's imports.
- The launcher never creates files in the game folder.

### 5.5 XGameRuntime — offline subset (M1)

- `XGameRuntimeInitialize/Uninitialize`.
- `XTaskQueue` + `XAsync` — a port from **libHttpClient** (Microsoft, MIT), which
  contains their implementation.
- `XSystem*` (device type, console ID — from the configuration).
- `XPackage*` (mount path = game directory; DLC = subfolders from the configuration).
- `XUser*` — a single local offline user (gamertag from the configuration).
- `XGameSave*` → `saves\`, `XPersistentLocalStorage*` → `pls\`.
- `XGameUI*` → stubs returning "cancelled/unavailable".
- `XStore*` → an "offline/unavailable" error (no faking of licenses).

### 5.5a "Empty" D3D12.X graphics — null device (M1)

Without this the game would exit on the first graphics call, before it manages
to load most of its data. That is why M1 includes a minimal `d3d12_xs`:

- The exports that create the device/queues return **dummy objects**, which
  accept every call, log it and return success with valid
  dummy output parameters (e.g. further dummy objects, descriptors,
  "signaled" fences).
- The game's Win32 window (the game creates it itself via `user32`) and a "present" that
  only clears the window to a single color each frame. **In M1 there is no game image**;
  the image is M2.
- Vtable layout: initially the method order from the public PC D3D12 headers
  (DirectX-Headers) + the Xbox methods appended at the end. Every virtual table
  additionally has spare slots leading to stubs that log the **slot
  number and interface**. Thanks to that, a report from a real title shows
  where the guessed layout diverges (input data for M2).

### 5.6 Logging and report (M1)

- `logs\xcl.log` — levels, thread, time.
- `logs\report.json` on exit: missing imports (static), called
  stubs (dynamic, with a counter and call site), unmapped
  paths, (M2) shader formats and missing D3D12.X methods.
- `xcl inspect <exe>` — a report without running: PE header, TLS, relocations,
  imports (also delay-load) with a status: implemented / fallback to the
  system / stub. Read-only.

### 5.7 D3D12.X → D3D12 graphics (M2 — direction, separate design round)

- Backend: **native D3D12** on Windows (Agility SDK, public). The architecture
  allows swapping `d3d12.dll` for **vkd3d-proton** (LGPL-2.1, D3D12 → Vulkan)
  in the future (Linux/Vulkan).
- `d3d12_xs.dll` exports and D3D12.X interfaces — signatures from the public
  docs.xbox.com reference; wrapper objects forwarding calls to
  D3D12 on PC; Xbox-only methods emulated or a no-op with a log.
- Presentation: console `PresentX`/frame events → DXGI swap chain
  (flip model) on the game's Win32 window.
- Shaders: parse the container; if there is a **DXIL** part → remove the Xbox
  parts, re-sign (validator from DirectXShaderCompiler) and create the PSO;
  if there is only hardware code → log + report (out of scope).
- The hardest topics, for a separate design: unified memory and GPU
  addresses assigned by the application, the vtable layout of the D3D12.X interfaces (the headers are
  under NDA).

### 5.7a Console shaders — RDNA2 recompiler (M2, decision of 2026-11-20)

The user pointed out (and this agrees with risk no. 1) that D3D12.X ships
shaders precompiled to the console GPU's microcode (RDNA2), not just DXIL.
No PC driver accepts raw ISA, and the user's card (RDNA4) has
a different instruction set — an **RDNA2 microcode → PC shader recompiler** is needed.

Stages:
1. **Capture** (done): every blob from PSO / root signature creation
   is classified (DXIL / DXBC / container with unknown parts / ELF /
   raw RDNA2), saved to `<data>\shaders\` and counted in `report.json`.
2. **RDNA2 decoder/disassembler** (done): tables generated from the official
   AMD Machine-Readable ISA specification (MIT) by the `scripts/gen_rdna2.py` script;
   validated on 23 812 instructions from the LLVM tests — 100% name and length match
   for all instructions that exist in RDNA2; `xcl disasm`.
3. Control-flow graph + structurization (EXEC mask → per-lane branches).
4. IR and instruction semantics (first those that occur in dumps from games).
5. Resource model: hardware V#/T#/S# descriptors → D3D12 bindless
   (SM 6.6 `ResourceDescriptorHeap`), format conversion at runtime.
6. Backend: HLSL → `dxcompiler`/`d3dcompiler_47`, result cache.
7. Shader inputs (user SGPRs, initial VGPRs) — these require data from real
   console containers (unknown parts) — hence stage 1 comes first.

**Console container format (done 2026-11-20, `gpu/xbox.rs`)** — determined on
a real game's shader archive (26 503 shaders: PS 12 101, MS 9 282, VS 2 236, GS 195, CS 2 689;
until 2026-11-22 the record scan missed the 2 634 compute shaders with effect-qualified names):

- DXBC: SFI0, ISG1, OSG1, PSV0, HASH, DXIL, PRIV. The DXIL part has an ordinary
  `DxilProgramHeader` (SM 6.6), bitcode = a 4-byte `NOBC`; between the header and the bitcode sits
  `XDAT` (+0x18): number of entries, table offset, entries (id, offset, size) counted from
  the program header. 0x12 = 16 B hash, 0x0D = XBCH, 0x07 = nested header (NOBC).
- `XBCH` v3: compressed size (+8), unpacked size (+0x10), zlib stream from +0x18.
- `FSSM` v0x101: size (+8); u64 code offset/size at +0xC0/+0xC8 (offset from +0x10);
  stage descriptors `0x000000C0_0002_AAHH` from +0x100 (every 0x88): HH = PAL hardware stage
  (2 ES, 3 GS, 4 VS, 5 PS, 6 CS, 7 = MS/NGG), AA = API stage (0 VS, 1 PS, 2 GS, 3 CS, 6 MS).
  A VS used with a GS has two XBCH (ES + VS); a GS has GS + VS descriptors (copy shader), but only
  one program — the copy shader is produced outside the FSSM. Unused fields = 0xDEADBEEF; in a PS
  the header contains a table of export variants (`exp` patches for RT formats).
- Code: one program per FSSM; it may have several `s_endpgm` (a shared "all
  pixels discarded" exit in PS, trampolines entered via `s_setpc`); constants are embedded in the code
  as `s_endpgm | u32 N | N dwords | s_endpgm` (addressed via `s_getpc_b64`).
  The code starts with two `v_nop` (VOP3 + literal) whose fields and literals embed
  the MD5 from the HASH part (bytes 0–2, 4–10, 12–15 are visible directly); a VS then continues with
  `s_swappc_b64 s[4:5]` — a call to the fetch shader (vertex attributes → VGPRs per ISG1).
- After the code: a table of entries (3, offset in code, 1, value, 8) – looks like a
  PC → source line mapping; then the shader name.
- ISA: RDNA2 + the gfx10.1 leftovers `v_mac_f32` (VOP2 0x1F/VOP3 0x11F), `v_madmk_f32`
  (0x20), `v_madak_f32` (0x21), `v_mad_f32` (VOP3 0x141) — `gpu/rdna2_xbox.rs`. The hardware
  fetches a literal when any VALU SRC field = 255 (including fields unused by the opcode).
  After that the decoder recognizes 100% of the corpus's 28.97 M instructions (395 mnemonics).
- Recompiler priorities: `xcl shaders <archive|folder>` — a histogram of how many shaders
  each mnemonic occurs in, and coverage (how many shaders use only the first K mnemonics):
  K=100 → 6.8%, 150 → 29%, 200 → 54%, 300 → 97.5%, 395 → 100%. Most frequent:
  `v_mul_legacy_f32` (13.3% of instructions, DX9 semantics: 0·x = 0), `v_fmac_f32`, `s_waitcnt`,
  `v_fma_f32`, `v_mov_b32`.

References: shadPS4 (GCN → SPIR-V, GPL-2.0 — only as a model, without copying
code), AnyPS5 (RDNA → SPIR-V for PS5, **GPL-2.0-only** — since 2026-11-20 XCL is also
GPL-2.0-only, so logic can be ported with an origin annotation; different backend: Vulkan/SPIR-V
and the PS5/SRT resource model),
Mesa ACO (MIT, RDNA2 semantics), RDNA2 ISA documentation (AMD), LLVM AMDGPU
(Apache-2.0 with LLVM exception — GFX10 buffer format table).

### 5.7b Recompiler — status (2026-11-20, `gpu/recomp/`)

**Execution model (exact without graph structurization):** every branch in AMD code is
scalar, i.e. common to the whole wave, so the program is a dispatcher loop over basic
blocks (`switch (pc)`); programs without branches are emitted linearly. SGPR/VCC/EXEC/M0/SCC
are ordinary variables computed identically by all threads (uniform), masks from vector
instructions via `WaveActiveBallot`, VGPRs per thread with writes masked by the EXEC bit,
cross-thread reads (`v_readlane`, DPP16/DPP8, `ds_swizzle`, `v_permlane(x)16`) via
`WaveReadLaneAt` (a variable index works on RX 9070 + DXC 1.8). Wave64/wave32 is detected
from the code (`detect_wave64`).

**Kernel mode (verification):** a `[WaveSize(64)]` compute shader — starting registers from a buffer,
final registers and exports to a buffer, flat memory (`mem`: addresses from V#/pointers).
`runner` (D3D12 on the high-performance GPU) + `dxc` (dxcompiler.dll from the Windows SDK or
`XCL_DXC_DIR`, signing via dxil.dll). Instruction tests on a real GPU (assembler
`rdna2::encode`): f32/u32/i32 ALU with VOP3 modifiers, SCC, loops, divergence via EXEC
(including a skipped block), SDWA (sel/sext/dst_sel/preserve), DPP16/DPP8, cross-lane,
`v_fma_mix*`, cube, `s_load`/`s_buffer_load`/`buffer_load`/`buffer_store`, typed reads
(`buffer_load_format_*`, `tbuffer_*`: 78 GFX10 formats, DST_SEL, unaligned addresses,
exact UNORM/SNORM range endpoints — DXC divides approximately), exports (with COMPR).
**The real console shader `VS_BlitGenAmbCube` runs on an RX 9070 with a correct
SV_Position** (`s_swappc_b64 s[4:5], s[4:5]` = a call to the fetch shader → attributes in VGPRs).

**Corpus (`xcl shaders … --recomp --compile N`):** 24 064 programs, 0 translation errors,
**7 442 (30.9%) translate completely**, DXC compiles 500/500 samples. The remaining barriers are
solely the graphics interface: `v_interp_*` (PS inputs) and `image_*` (textures).

**Next:** (1) VS/PS modes: VS inputs from ISG1 instead of the fetch shader, `v_interp_p1/p2/mov`
→ PS inputs from ISG1 (attribute index = register), `exp pos/param/mrt/mrtz` → HLSL outputs
(OSG1), (2) resource model: decoding T#/S# and mapping console GPU memory to D3D12
resources (bindless SM 6.6 + "flat" buffers), (3) LDS (`ds_*` → groupshared in CS),
(4) HLSL compactness (5.6 GB for the whole corpus — dead register elimination, merging
expressions) and structurization for performance, (5) DXIL cache.

### 5.8 Audio (M3 — direction)

- By default: the console XAudio2 API → Windows' **system XAudio2.9**; **XMA2**
  sources (`WAVE_FORMAT_XMA2`) decoded to PCM by **FFmpeg libavcodec**
  (it has an XMA2 decoder, LGPL-2.1+); porting `XmaDecoder` from WinDurango is ruled out (LGPL-3.0 is incompatible with XCL's GPL-2.0-only).
- AcpHal / APU / SHAPE (the console's audio hardware, a public reference exists)
  → stubs + report first; emulation of SHAPE-XMA contexts if titles
  use them directly.

**Done 2026-11-20 (`hle/xaudio2.rs`)**: `XAudio2Create` / `XAudio2CreateWithVersionInfo`
create the real Windows engine (flags outside 0x2000|0x8000|0x10000 and the console core mask
are dropped, processor = `XAUDIO2_DEFAULT_PROCESSOR`), vtables patched in place: IXAudio2 #5
CreateSourceVoice (XMA/XMA2 → PCM16, same channels and sample rate; any other format that
Windows will not accept, e.g. Opus 0x704F, also → PCM16), #7 CreateMasteringVoice (console
device ID → default; `CO_E_NOTINITIALIZED` → `CoInitializeEx(MTA)` and retry —
the console does not require COM; then a retry with 0/0 and the GameEffects category), IXAudio2SourceVoice
#21 SubmitSourceBuffer (for such voices the buffer = silence of length PlayBegin+PlayLength or
estimated from SamplesEncoded/BlockCount·BytesPerBlock or nAvgBytesPerSec; context, flags and
loops preserved → callbacks on the title's timeline), #18 DestroyVoice. The remaining exports → the system
`xaudio2_9.dll` (the HLE module's `fallback` list). Statistics: `report.json → audio`.

**Data from a real game (`audio.rpf`, 28.5 GB, AWC "ADAT" containers)**: no XMA2 — AWC codec
0x0B = **Opus** 48 kHz (2048 B blocks: `u16 LE length + Opus packet`, 20 ms frames, TOC
CELT/Hybrid fullband; music streams: 512 KB blocks). Conclusion: for this title an XMA2 decoder
is not needed; the game either decodes Opus itself (PCM → XAudio2, works), or uses
the console's hardware Opus decoding — to be checked in the exe's imports (`xcl inspect`).
Windows has an Opus decoder in Media Foundation (`MSOpusDecoder.dll`) — a candidate for the backend.

### 5.9 Input and the rest (M4 — direction)

- GameInput → **GameInput on PC** (Microsoft, redistributable); XInput fallback.
- Console DirectStorage → DirectStorage on PC or our own implementation
  (file + zlib). BCPack — no public decoder (risk).

### 5.10 Configuration

A TOML file per title in `%LOCALAPPDATA%\XCL\<title>\xcl.toml`: console type,
mounts, DLL aliases, stub return value overrides, log level, gamertag.
Sensible defaults — the game should be runnable without writing a config.

## 6. Stages and success criteria

| Stage | Scope | Done when |
|---|---|---|
| **M0** | Repo skeleton, CMake+Ninja+MSVC, unit tests, local CI (`ctest`). | `cmake --build` + `ctest` pass. |
| **M1** | Loader, resolver, stubs, kernelx, VFS, offline XGameRuntime, null D3D12.X (§5.5a), log/report, `inspect`. **= the user's first success (no image).** | The test program (§7) passes all checks via `xcl run`; the game folder is unchanged byte for byte; `xcl inspect` works for any x64 PE. **Acceptance by the user** (not a completion condition, just the next iteration): a run on their own title, report → further fixes. |
| M2 | D3D12.X → D3D12 (separate design). | The test program renders through our D3D12.X. |
| M3 | XAudio2 + XMA2. | The test program plays an XMA2 clip. |
| M4 | GameInput, DirectStorage. | A gamepad controls the test program. |

## 7. Testing

- **Unit tests** (doctest, MIT): PE parser, relocations, resolver
  policy, stub generator, path translation, copy-on-write, TLS.
- **Test program `xcl_testtitle.exe`** — our own "console title":
  linked against import libraries generated from `.def` files with console
  DLL names (`kernelx.dll` etc.), so its import table looks
  like a Series game's; it will not run on plain Windows — only through `xcl`.
  It checks: static TLS in multiple threads, C++ exceptions and SEH, named
  threads, `VirtualAlloc` with console flags, `XMemAlloc`, reading files
  from the mounted folder, a write attempt (→ overlay), `XTaskQueue`/`XAsync`,
  `XUser`, `XGameSave`, `LoadLibrary`/`GetProcAddress` of a "game" DLL (a test DLL
  loaded by our loader), a call to an unimplemented function
  (→ stub, the program keeps running). Result: exit code + log.
- **Integrity test:** before and after every run, a hash of all the
  files in the test game's folder + a file list — they must be identical.

## 8. Ready-made sources and components

| What | Source | License | Use |
|---|---|---|---|
| Console API signatures (D3D12_XS, XGameRuntime, XMA2, SHAPE, DStorage, GameInput) | docs.xbox.com (public, `llms.txt` index) | documentation | declarations written by ourselves |
| kernelx, D3D11.X, XMA implementations | github.com/WinDurango/WinDurango | LGPL-3.0 | reference only (since XCL's license changed to GPL-2.0-only, its code must not be ported) |
| RDNA → SPIR-V recompiler (PS5): descriptor decoding, images, interpolation, structurization | github.com/boykopovar/AnyPS5 | GPL-2.0-only | port of the logic with an origin annotation |
| XTaskQueue/XAsync | github.com/microsoft/libHttpClient | MIT | port |
| API hooks | github.com/microsoft/Detours | MIT | VFS, image visibility |
| D3D12 headers | github.com/microsoft/DirectX-Headers, Agility SDK | MIT / MS | M2 |
| DXIL validator/signing | github.com/microsoft/DirectXShaderCompiler | NCSA/MS | M2 |
| D3D12 → Vulkan (option) | github.com/HansKristian-Work/vkd3d-proton | LGPL-2.1 | M2+ |
| XMA2 decoder | FFmpeg libavcodec | LGPL-2.1+ | M3 |
| PE loader reference | github.com/fancycode/MemoryModule | MPL-2.0 | reference |
| Tests / config / JSON | doctest, toml++, nlohmann/json | MIT | M0+ |

**Project license:** GPL-2.0-only (since 2026-11-20, the user's decision; previously
LGPL-3.0-or-later). Reason: use of AnyPS5 code (GPL-2.0-only). Consequences: code from WinDurango
(LGPL-3.0) only as a reference; dependencies must be GPL-2 compatible (MIT/BSD/Zlib/LGPL-2.1+;
not: Apache-2.0 without an alternative, GPL-3/LGPL-3); DXC (`dxcompiler.dll`/`dxil.dll`) is loaded
from the user's Windows SDK, not distributed with XCL. List: `THIRD_PARTY.md`.

## 9. Risks

1. **Shaders without DXIL** — whether Series games contain DXIL alongside the hardware
   code is described by documentation under NDA. If not → graphics for such a
   title are blocked. Checked in M2 via the shader format report.
2. **The D3D12.X vtable layout** is not public → it has to be determined
   from binary analysis; a mistake = a crash. Affects M2.
3. **GPU memory model** (unified memory, GPU addresses = CPU addresses) —
   possible performance problems on cards with separate VRAM. M2.
4. **Static TLS and C++ exceptions** in a manual loader — tested
   first in M1.
5. **BCPack / hardware decompression** in DirectStorage — no public
   decoder.
6. Anti-tamper protections in games — out of scope.
7. Hardware requirements: the console has 16 GB of unified memory; a PC needs
   a comparable amount of RAM + VRAM.

## 10. Who decided what

**User:** goal = the game starts and reads its files; a general layer, not
tailored to one title; **nothing in the game folder may be deleted or changed —
only single files copied**; preference for DXVK/DXMT (considered —
§4, does not fit D3D12.X).

**Change after approval (user):** **Rust** as the language instead of C++, without
a separate implementation plan. Consequences: a cargo workspace instead of CMake;
hooks via the `retour` crate (BSD-2) instead of Detours; XTaskQueue/XAsync
written in Rust following the libHttpClient headers (MIT) as the specification;
tests via `cargo test`; the test program also in Rust ("console" imports
via `#[link(kind = "raw-dylib")]`). Static TLS: a so-called "TLS carrier" —
a minimal DLL generated on the fly with a TLS directory the size of the game's
template, which Windows loads (it manages the slot for all threads), and XCL
copies the game's template into it at the start of every thread. The launcher runs
the game in a child `xcl.exe` process with the game's command line (argv[0] = the game exe).

**Designer (subject to correction):** approach B; a single `xcl.exe` process with
static modules; x64 only; LGPL-3.0 license (since 2026-11-20: GPL-2.0-only, §8); offline XGameRuntime; XStore without
faking licenses; native D3D12 as the backend; XAudio2 → system XAudio2.9
+ FFmpeg for XMA2; data in `%LOCALAPPDATA%\XCL\`; the name XCL; project
in this repository; **null D3D12.X in M1, image only in M2**;
Windows only (no requirement of Wine compatibility); real titles are tested by
the user, and the implementer works from the reports; a data-driven resolver
(ordinal database).

## 11. Left to the implementer

The exact technique for TLS and image visibility (hook vs PEB entry), the level
of the VFS hooks (Win32 vs `Nt*`), the implementation of the stub thunks, the log library,
the repo directory layout, the `report.json` format, the default return values of
stubs beyond the "HRESULT → E_NOTIMPL" rule.
