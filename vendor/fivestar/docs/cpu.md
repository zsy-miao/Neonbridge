# AMD-only instructions

The Xbox Series CPU is AMD Zen 2, and the game executable uses instructions only AMD CPUs have. On a CPU without them
each one raises #UD (`STATUS_ILLEGAL_INSTRUCTION`, 0xC000001D). The loader emulates them in its exception handler;
on a CPU that has them they run natively and the handler never sees them.

| Instruction | CPUID bit | Intel CPUs |
|---|---|---|
| INSERTQ, EXTRQ (SSE4a) | Fn8000_0001 ECX bit 6 | none |
| CLZERO | Fn8000_0008 EBX bit 0 | none |
| MONITORX, MWAITX | Fn8000_0001 ECX bit 29 | none |
| CLWB | Fn0000_0007 EBX bit 24 | some (Xeon Scalable, recent client parts); not e.g. Coffee Lake |

## Census

`llvm-objdump -d` (LLVM 18) over every executable section, sites grouped by the `.pdata` function around them
(chained unwind entries followed to the primary function). The package DLLs (`Bink2Scarlett`, `XboxBvhBuilder`,
`amd_fidelityfx_*`, `libHttpClient.GDK`, `XCurl`, the MSVC runtime) have none. The executable:

| Instruction | Sites | Where |
|---|---:|---|
| INSERTQ (immediate form) | 19 | 11 functions, e.g. 0x1407f1560 and 0x140f740a0 (4 each), and 2 sites in leaf code without `.pdata`; fields of 8 to 24 bits, xmm0 to xmm14 |
| CLZERO | 10 | one leaf routine (sites 0x144e8aa70 to 0x144e8abb4) that clears a structure in ranges of 0x380 to 0x1b80 bytes |
| CLWB | 8 | write-back loops over a range: 0x14532f140 (5), 0x144ecc830, two leaf loops |
| MONITORX, MWAITX | 2 + 2 | the fence wait 0x144fb0ad0, with the timer (ECX = 2, EBX = timeout) |
| EXTRQ, MOVNTSD, MOVNTSS | 0 | |

Hot paths: the fence wait spins MONITORX/MWAITX while it waits; every CLZERO and CLWB sits in a loop that steps 64
bytes per pass, so a range costs one fault per cache line unless the loop is recognised (below). How often the INSERTQ
functions run is not known.

## Emulation

`AmdInstructionHandler` (`src/FiveStar.Loader/Cpu`) runs first in the fault reporter's vectored handler, which the
loader installs with `--run` and keeps in front of the game's own handlers: when mapped code adds a first handler
(Bink does at every video), the loader registers its handler again before it removes the old registration, so a
fault on another thread meanwhile is never without it (it was, briefly, and a CLWB that faulted then ended the
process with 0xC000001D, issue #13). That still left one gap: a fault whose dispatch had passed the head of the list
before the new registration went there reached the old one only after it left, and a CLZERO 18 minutes into a boot
ended the process the same way. So the loader also registers the emulation alone as the last vectored handler (the
backstop, never removed): whatever runs first and continues the search, every emulated instruction (CLZERO,
MONITORX/MWAITX, CLWB, EXTRQ, INSERTQ) reaches it. For 0xC000001D it decodes the bytes at
RIP and accepts only these encodings; anything else continues to the fault report and the game's handlers.

| Instruction | Emulation |
|---|---|
| EXTRQ, INSERTQ | the bit field on the low quadword of the CONTEXT's XMM register (immediate and register forms, REX registers); the high quadword is cleared, as Zen hardware does |
| CLZERO | 64 zero bytes at rAX rounded down to the line (EAX under 67h) |
| MONITORX | nothing |
| MWAITX | `pause` when the timer is on with at most 4000 ticks, else `SwitchToThread` |
| CLWB | nothing (no persistent memory on the PC) |

RIP then moves past the instruction and the thread continues. The handler allocates nothing and takes no lock; the
first fault at a site logs `cpu: emulating <instruction> at <image+rva>`, and the loader's summary at exit gives the
count per instruction (per site in the log file). Game code is never changed.

**Cache-line loops.** When the instruction heads `add p,64; cmp p,end; jb/jbe` back to itself (CLZERO with p = rax,
CLWB `[p]`), one fault runs every pass but the last: CLZERO clears those lines, p moves to the last pass and RIP
stays. The last pass faults again and is emulated alone, so the hardware runs the final `add`, `cmp` and branch and
the flags are the loop's own. This covers 6 of the 10 CLZERO sites and all 8 CLWB sites.

## Checking it on an AMD CPU

At start the loader logs which of the four the host lacks. `fivestar-loader --cpu intel` reports all four missing
and runs a self-test through the installed handler: each case is generated into the loader's code heap twice, plain
(native on AMD, emulated on Intel) and behind a `ud2`, which faults on every CPU and which the handler treats as a
marker only inside that code. Both results must match each other and the reference. `CpuEmulationTests` runs the
same self-test behind a vectored handler of its own, plus the decoder and the emulation on hand-made CONTEXTs.

## Unknown

- The cost on an Intel CPU: each emulated instruction is a kernel exception round trip (microseconds). The INSERTQ
  functions and the 4 CLZERO sites the loop rule does not cover fault once per execution.
