# Debugging

Three tools find faults. Each one sees a different set of them.

| Tool | Sees | Use it for |
|---|---|---|
| The loader's fault reporter (`fivestar-loader --run`, [loader.md](loader.md#running-the-executable)) | hardware exceptions whose first 12 frames include a mapped image, from inside the process: image RVA, `.pdata` function, code bytes, registers, the last traced shim calls | faults in game code. It is the first thing to read. |
| `fivestar-dbg` (`src/FiveStar.Debugger`) | every exception in the process, from outside it, at first and second chance: module loads, debug output, symbolised stacks, minidumps, the page heap | faults inside coreclr, ntdll's heap manager, a driver or the fault reporter itself, heap corruption, and crashes that leave no log |
| `--d3d12-debug`, `--d3d12-gpu-validation` ([d3d12x-shim.md](d3d12x-shim.md#debug-layer)) | D3D12 API misuse and GPU faults: debug-layer messages and the DRED report after a device removal | `DXGI_ERROR_DEVICE_REMOVED`, hangs on the GPU, wrong descriptors or barriers |

## fivestar-dbg

```powershell
fivestar-dbg [options] [--] <program> [args...]
fivestar-dbg --debug-heap --break-on AccessViolation --dump-on-crash crash.dmp `
  --symbols "srv*C:\symbols*https://msdl.microsoft.com/download/symbols" `
  -- fivestar-loader --load $dump\game.exe --run --log run.log
```

| Option | Meaning |
|---|---|
| `--debug-heap` | ntdll's debug heap and the full page heap, for this process only (below) |
| `--break-on <code,...>` | a full record (registers, stack) already at first chance; hex codes or names (`AccessViolation`, `HeapCorruption`, `StackOverflow`, ...) |
| `--symbols <path>` | folders and `srv*<cache>*<url>` entries; default `_NT_SYMBOL_PATH`, else exports only |
| `--dump-on-crash <file>`, `--full-dump` | a minidump at the first second-chance or `--break-on` exception; with all process memory |
| `--log <file>` | a copy of the output |
| `--loader-log <file>` | the fivestar-loader log; default the program's own `--log` value |
| `--children` | also debug the processes the program starts (`DEBUG_PROCESS`) |
| `--timeout <s>` | end the program after s seconds (exit code 124) |

The exit code is the program's. The tool builds for NativeAOT (`IsAotCompatible`); it calls kernel32, ntdll and
System32's dbghelp through `LibraryImport`.

### Behaviour

- **Exceptions.** Each first-chance exception is one line: chance, code and name, address, `module+offset`, symbol, and
  for an access violation the access and address. After 20 of one code they are only counted; the totals come at the
  end. First-chance exceptions go back to the program (`DBG_EXCEPTION_NOT_HANDLED`). A second-chance exception gets a
  full record with every thread's stack.
- **Breakpoints.** ntdll's startup breakpoint passes silently. Any later one (a heap check, a verifier stop, an
  `__debugbreak`) gets a full record and is stepped over, as a debugger's `g` does.
- **.NET hosts.** An unhandled exception in a .NET process (the loader is one) ends it through the runtime's fail-fast
  without a second chance, so `--break-on` is what gets the stack and the dump.
- **Stacks.** `StackWalk64` unwinds through the `.pdata` of every image dbghelp knows. JIT code has no image; the walk
  stops at the first frame outside an image.
- **Images the loader maps.** The OS never reports them. At each report the tool reads the new lines of the loader log
  (`== loading <path>`, `mapped <name> @ 0x<base> (size 0x<size>`, `unloaded <name>`), registers those images with
  dbghelp and names addresses in them as `game.exe RVA 0x1234 (mapped)`.
- **Symbols.** A name dbghelp takes from the export table is marked `(export)`: it is the nearest export, possibly a
  different function. System32 has no `symsrv.dll`, so for `srv*` entries the tool reads the image's CodeView record
  and downloads `<pdb>/<GUID><age>/<pdb>` into the cache (symstore layout) the first time a module appears in a report.

### Debug heap

Nothing system-wide changes, and no registry key is written:

| Mode | How | Catches |
|---|---|---|
| default | `_NO_DEBUG_HEAP=1` in the program's environment, which turns off the debug heap ntdll enables for every process started under a debugger | nothing extra: the heap behaves as in a normal run |
| `--debug-heap` | no `_NO_DEBUG_HEAP`, so tail checking, free checking and parameter validation stay on; and at the create-process event, before any user-mode code runs, `FLG_HEAP_PAGE_ALLOCS` (0x02000000) is set in the debuggee's `PEB.NtGlobalFlag` (offset 0xBC). ntdll then loads verifier.dll and logs `page heap enabled with flags 0x2` (full page heap) | an overrun of a block whose size is a multiple of 16 faults on the writing instruction (guard page); other overruns and writes after free are reported as a verifier stop at the free |

The full page heap costs at least two pages per allocation; a long game run under it needs memory to match.

### Output

```text
[   0.025] 40756 page heap on
[   0.027] 40756/54844 debug: Page heap: pid 0x9F34: page heap enabled with flags 0x2.
[  12.435] 40756/54844 first-chance c0000005 AccessViolation at 0x7ff975a9ca06 ntdll.dll+0x16ca06 ntdll!_memset_spec_avx_ermsb+0xc6 (write at 0x2f9a4767000)
[  12.437] 40756 exited with 0xc0000005 (c0000005 AccessViolation)
```

Tests (`tests/FiveStar.Debugger.Tests`, Windows): the test assembly is also the debuggee. An access violation in ntdll
is reported with module, symbolised frame and a minidump; a one-byte heap overrun passes unnoticed on the normal heap
and faults on the write under `--debug-heap`.

## Transient pool watch

`fivestar-loader --watch-transient` follows the game's transient-memory pool, the per-frame memory its command-list
contexts take constants and small objects from (set up at 0x14247ac20; globals pool base 0x1475DF1B8, block records
0x1475DF1C0, count 0x1475DF1C8, block size 0x1475DF1CC). Each 0x58-byte block record holds the block's address
(+0x10), its retire token (+0x18) and a list (+0x48) of 0x18-byte nodes {next, pooled allocator or list, token}
stored inside the blocks: 0x140188430 links one when a context takes a pooled allocator, and 0x144e9b480 (or
0x1450b8970) walks the list when the block is taken again, then starts a new one in the block's last 0x18 bytes. The
story-load faults at 0x144e9b4b2 read such a list with garbage nodes (0x2, 0x200000001).

Every 5 ms a thread walks every list; a next pointer outside the pool or an object pointer outside user space, seen
on three polls in a row, is logged once with the block, the node and the 0x60 bytes around it, followed by every
queue as last seen ([queue progress](d3d12x-shim.md#queue-progress)). Root CBV, SRV and UAV addresses (list slots 41
to 46) that fall in the pool are kept in a ring of 4,096 with the list's pipeline; the report lists the newest 24 in
the corrupted block, which names the GPU work that used it. The game's code is read, never changed.

With `--d3d12-breadcrumbs` as well, the watch copies every recording's marker before each check, keeps the copy from
the last clean check, and copies them again after the check that first sees the corrupted node. The report lists the
operations between the two (done, or still running at the second copy) with their pipelines, which bounds the GPU
work that wrote the node to that window. Each marked draw, dispatch and ExecuteIndirect also has its user data
searched for dword pairs whose 48-bit value reaches into the pool (a root descriptor's address, or a V#'s base with
its stride and record count in the next dwords); a ring keeps 65,536 of them with the operation and its pipeline, and
the report lists those of the window's operations that reach the corrupted block, then the newest of any operation.
Buffer UAVs in descriptor tables count too: each UAV's game range is kept by the desktop index its record names,
and a user dword that points at the window's view records is read as a table whose first 16 records are checked;
their entries show the record's words as the program reads them (desktop index, stride, num_records, OOB_SELECT).

**Lifetime check.** With both flags, each recording whose operations reach a transient block (root addresses, user-data V#s, table UAVs) is kept per block, and a range over more than 8 blocks (the game's whole-pool raw view) in a separate ring. When a block record's retire token (+0x18) changes, the game has taken the block again; every kept recording of it that has not completed on its queue's shim fence, or was never submitted, is logged as `[lifetime]` with its queue, how many submissions behind it is, the last slot-74 retire write and its pipelines (the first 24), and counts follow every 10 s.

**Store trace.** `--trace-pool-stores` gives programs made on demand a FIVESTAR_STORE_TRACE id (and their own program cache identity). Their global stores and atomics into up to 8 address ranges (the transient pool; the game heap allocation that holds the block records, the fixed-size pool pages and their descriptors), and their buffer stores and atomics through up to 8 views over the pool, append {program, address or offset, value} to a ring of 65,536 entries at window offset `PoolStoreTrace.TraceBase` (rdna_prelude TRACE_BASE). A transient report lists the newest entries into the corrupted block; global stores outside the pool are logged as `[store trace]` as they appear (the first 3 per program, 64 in all), and a structured loop that reaches the loop guard logs its program once (and again at 1,000 hits). Boot 14 named the writer of the story-load corruption this way: InstanceCulling~CS_Prop_BatchCull, whose 20-byte DRAW_INDEXED InstanceCount increments, through the game's whole-pool raw view, ran past its argument region into a block's end node. The cause was the translator: the program sets s43 = s60 + 4 (the base of its argument array) and adds index * 20, but a v_cmp into s[42:43] on another path made the translation read s43 as that mask's high half (mask tracking is by emission order), so the atomics landed near the pool's start. The high dword of a wave64 mask is now a variable every mask write sets (cs_mask_high_half_path_w64); boot 18 ran past 200 s without corruption. Constant-buffer loads past their descriptor (they read 0) and loops that reach the loop guard are also logged.

## Hang watchdog

The game ends itself when its main loop stops for 30 s. A watchdog thread (0x142215a60) checks once a second that
the heartbeat at 0x1471B6338 moved; the main loop writes a QueryPerformanceCounter value there each frame
(0x1422159f0, called from 0x140120790, 0x140c9e350 and the wait loop 0x141c82b20). After 30 checks without a move it
calls the handler at 0x1471B6348, if one is set, then writes to address 0. A pause flag (0x1471B6330, set for event
0x24 and cleared for 0x25 by 0x1422159c0) stops the count.

During the story load the main state machine (object 0x1471273A0, state at +0x10, next state at +0x12) enters state
3, whose entry 0x140812620 goes straight to state 4 when the loading screen flag 0x147453ED4 and byte +0x1B3 of the
session object at [0x14AAE3B70] are set. Otherwise, the usual case for a fresh load, it stops the loading screen
first: 0x1423cf6a0 sets the handler to 0x1423cf620, which reports `GPU Loc Sig` and writes 0x4D8BD421 to address 0,
then waits without a limit (0x1423ceeb0) for the loading-screen render thread (0x1423cefe0) to signal the end of a
frame (event 0x14A210CF8). That thread renders through 0x1443c38a0 and waits for each of the five `Subrender:%02u`
workers (0x1443c4140). While it waits no heartbeat is written, so a loading-screen frame that takes more than 30 s
ends the game with `GPU Loc Sig`.

Two causes were seen. Workers that fault in the game's pool allocators (the transient-pool corruption) never return.
And workers slept in the loader: a small compute stand-in waited up to 250 ms at its first use even while its
programs were still being made on demand (55 to 73 s on average during the story load), and a loading-screen frame
met hundreds of them, so all five workers slept for more than 30 s. A first use now waits only when the programs are
ready. Waits a game thread can reach inside the loader register with `BlockingWaits`, so a stack that ends in JIT
code still names the wait (`(waiting in ... for N ms)`). The stall line gives the time spent in each since the
heartbeat last moved, over all threads (a wait still running shows in its thread's stack instead), then the totals
since the start: those are a whole boot's, so seconds of them say nothing about one stall.

`fivestar-loader --watch-hang` reads these globals every 100 ms and logs the main state, the loading-screen flags,
the session byte, the pause flag and the handler when they change, a heartbeat that moves again after 2 s or more, and
the loader counters and every thread's stack (64 frames, with thread names) when the heartbeat has stood still for 5,
15 and 25 s. With it the loader keeps shims on the game's CreateEventW/A, CreateSemaphoreExW, SetEvent,
ReleaseSemaphore, CloseHandle and WaitForSingleObject(Ex) (`WaitObjectShims`, removed otherwise), and the dump lists
every thread blocked a second or more in one of those waits: the handle, the timeout, how the object was made (event
kind or semaphore counts, creating thread and time, the game frames of the creating call) and whether it was signaled
since, how often and by which thread last (`WaitObjects`). A stall whose waiter sits on an object nobody signals any
more (a lost wake-up) then names that object and its creator. For each thread in the game's fence wait (0x144fb0ad0) it also names what the wait depends on
(`StallCulprit`): the queue that was asked to signal that fence to the awaited value and the last six executions it
was given before that Signal, newest first, each with its last pipeline and whether it finished, and any Wait on another
fence among them; or, when no queue was asked yet, the fence's last Signal. The queues keep their last 1,024 calls in
any case (`QueueProgress`), so this costs nothing until a stall. The game's code is read, never changed.

## Memory map

`fivestar-loader --memory-map <s>` walks the address space (VirtualQueryEx) every s seconds and logs `[memory]` lines:

| Line | Holds |
|---|---|
| `#n` | private commit by owner: `XMemVirtualAlloc` (plain allocations of that shim), `XMemAlloc` blocks, heaps (process heaps and blocks with a heap segment signature), game images, everything else as `anonymous`; the managed heap's commit (part of the anonymous); mapped and image commit |
| `private by protection` | 0x4 read-write, 0x404 read-write write-combined (upload and custom heaps), ... |
| `anonymous by size` | counts and bytes per size class, and the committed sizes that hold the most |
| `heaps` | HeapSummary of every process heap |
| `heap segments by heap` | the private bytes labeled heap, by the heap each segment belongs to (HEAP_SEGMENT.Heap), top 5, each named: the process heap (every CRT's malloc, the game's shipped one and the loader's native memory), made at a game frame (`HeapCreate` from mapped code passes through `HeapShims`), or made by a system or driver DLL. A 15.6 GB machine held 9.02 GB in 601 heap allocations after 18 minutes with nothing to say whose (issue #13) |
| `since the last map` | new and gone allocations, by owner and size |
| `largest` | the 20 largest: base, owner, committed of reserved, protection; an anonymous one with the modules its first page points into (`[nvgpucomp64.dll x3]`) |
| `gpu memory` | QueryVideoMemoryInfo per adapter: local and non-local usage and budget |

A walk of 200,000 to 300,000 regions takes 0.4 to 1 s. `--min-free-memory <GB>` and `--max-commit <GB>` poll the machine
every 0.25 s and end the process with a last map (exit code 5).

**Story load, before the compile budget** (RTX 4090, 64 GB, programs on demand): private 17 GB at 47 s, 26 GB at 96 s,
54 GB at 147 s, 60 GB at 165 s; the growth is anonymous memory in power-of-two regions, almost fully committed (8 GB,
4 GB x4, 2 GB x5, 1 GB x4). They are the NVIDIA compiler's arenas: six driver compiles of 1 to 3 MB mesh-shader
pipelines ran on the build threads (stacks in nvgpucomp64.dll under CreatePipelineState) from 100 s on, each growing
its arena 1, 2, 4, 8 GB. Everything else stayed flat: heaps 1.5 to 2 GB, `XMemVirtualAlloc` 4.5 GB, game images
0.2 GB, managed heap 1 to 1.5 GB, write-combined 5.2 GB. [d3d12x-pipelines.md](d3d12x-pipelines.md#stand-ins) has
the compile budget.

**Write-combined private memory is video memory.** On the RTX 4090 the driver maps device-local allocations into the
process as committed private write-combined regions (0x404, often 32 MiB): 400 default-heap textures of 1 MB added
400 MB of write-combined private memory, 400 MB of private bytes and 400 MB of GPU local memory, and 2 MB of working
set. A cold boot's grew from 5 to 12.6 GB after the story load step for step with GPU local memory (non-local flat).
It is charged to commit but not backed by RAM, so the sample summary names it inside private bytes, and the compile
ceiling leaves it out.

**Commit by category.** Each sample line splits private bytes (`CommitAccounting`): the video memory mirror
(write-combined, of it the placed texture heaps), the managed heap's commit, the game memory buffers' spans, and the
rest, the driver's and native memory, beside the running compiles' expected arenas. Outside private bytes it gives the
XMem pages (pagefile-backed sections, of them the spans section heaps are open over) and the helper processes.

Two of the loader's own shares are kept down. A spare command allocator (lent while the game's allocator backs another
recording list, `SpareAllocators`) holds the memory of its largest recording; one not lent over 120 resets of its game
allocator is released, counted in the sample line's `command objects`. The managed heap runs with
`System.GC.ConserveMemory` 5 (the loader's project file): the GC compacts the large object heap once it is half free
space rather than keep the fragmented commit.

## Unknown

- Whether ntdll keeps honouring `PEB.NtGlobalFlag` set at the create-process event in later Windows releases (checked
  on Windows 11 build 26200).

## Pool page watch

`--watch-pool-pages` (`PoolPageWatch`) looks for the GPU write that corrupts the game's small-block pools (the pool
free at 0x14540abb0 and the allocation at 0x144dac530 fault on a garbage header). Their 16 KiB pages live in the
1.1 GiB coherent graphics heap (XMemVirtualAlloc protect 0x22004) next to GPU data, so a GPU write out of range lands
there. Header (`PoolPageHeader`): the first qword packs the first free block's offset (low dword), a block count
(bits 32 to 47) and a counter each compare-exchange moves on by one (bits 48 to 63); +8 and +16 link the page into
its pool's lists, +34 flags the partial list, and each free block's first dword is the next one's offset.

Every 2 s a thread scans the coherent heaps at a 16 KiB stride for headers that read as one; a page counts once its
counter was seen moving. Every 100 ms it reads each page's header and the first free block's next-offset, through
ReadProcessMemory (a plain load faulted when the game freed an allocation between two scans). The pools themselves are
learned from the pages' list links (pool + 104) and their current page (+144) must stay 0 or a page. A change the
allocator cannot make (offsets outside the page, the counter not moved forward by 1 to 4,096) read the same twice is
logged with the page, its first 64 bytes and, per queue, the executions retired since the last check (their last
pipeline, and their dispatches with `--d3d12-breadcrumbs`). Cost: the scan reads 64 bytes of each of the heap's 73,000
pages (well under a millisecond of reads every 2 s); a check makes one or two ReadProcessMemory calls per watched page (up to 16,384) on its own
thread, with no GPU work and nothing on the submitting thread. A write between two checks is caught with the
executions of that window, not the exact dispatch; breadcrumbs narrow it.
