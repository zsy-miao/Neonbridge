# Set up ProjectScarlett (GTA VI) on Linux

This guide stages GTA VI 1.0.2.1 (`ProjectScarlett`) with TuxBox's providers and starts it on Valve's stock Proton Experimental. The furthest recorded run got into the story intro, where GPU queues stalled. The game isn't playable. [What to expect](#what-to-expect) · [Title progress](PROGRESS.md) · [Run record](../evidence/projectscarlett-gameplay.json)

You supply the game package. TuxBox doesn't include any game files. The recorded runs use package `RockstarGamesInc.28872443D20A7` 1.0.2.1, whose `game_scarlett_master.exe` has SHA-256 `e95020b2fa4a721db3df59fdecf75621730ac559d8835252f6c3b658d5a08993`. Other builds may need different providers or shader requests.

## Requirements

- The project's Linux build tools: Python 3.10 or newer, Rust with the `x86_64-pc-windows-msvc` target, `clang-cl`, `lld-link` and the xwin Windows SDK. [Build requirements](../examples/lanes/README.md#build-requirements)
- Proton Experimental from Steam and `umu-run`.
- Microsoft's Linux DXC release with `dxc` and `dxv`, for shader preparation and runtime translation. Arch's `directx-shader-compiler` 1.10.2605.37 fails on a minimal compute shader. Use Microsoft's release instead. [DirectX Shader Compiler releases](https://github.com/microsoft/DirectXShaderCompiler/releases)
- The Windows x64 DXC release (same-release `dxcompiler.dll` and `dxil.dll`) for generating rectangle geometry shaders inside the game process. The Linux `dxc` executable alone does not supply these DLLs. Use [Microsoft's official DXC releases](https://github.com/microsoft/DirectXShaderCompiler/releases).
- A C compiler and Vulkan headers. The launcher builds TuxBox's constant-buffer Vulkan layer on first use.

Run `python tuxbox.py inspect /path/to/package` to confirm the package. It should report one executable entry, `GTAO  Scarlett  game_scarlett_master.exe  present`.

## Build the providers

```sh
python -m pip install -r requirements.txt
python build.py
```

To build only what ProjectScarlett loads, run `python build.py <target>` for each of `xmem`, `d3d12`, `xg`, `xapu`, `acphal`, `xcrdapi`, `gameinput` and `xgameruntime`. Some Linux distributions currently need the build workarounds described in [issue #4](https://github.com/brcly/TuxBox/issues/4).

## Stage a working copy

Keep the original package unchanged and launch a copy. The package doesn't contain any of the provider DLL names, so TuxBox's copies are added beside the game files rather than replacing them.

```sh
cp -a --reflink=auto /path/to/GTA6.Xbox.v1.0.2.1 /path/to/scarlett-run
cp build/xmem-smoke/{xmem,tuxbox_xhttp}.dll \
   build/d3d12-smoke/d3d12_xs.dll \
   build/xg-provider/xg_xs.dll \
   build/xapu/{xapu,chad}.dll \
   build/acphal/AcpHal.dll \
   build/xcrdapi/xcrdapi.dll \
   build/gameinput/GameInput.dll \
   build/xgameruntime/xgameruntime.dll \
   /path/to/scarlett-run/
```

On a copy-on-write filesystem such as Btrfs or XFS, `--reflink=auto` copies the package almost instantly without using more disk space. If you use a hard-link copy (`cp -al`) to save space, never write into the linked game files. Writing to one also changes the original package.

| Provider | Build output | Loaded by |
| --- | --- | --- |
| `xmem.dll` | `build/xmem-smoke/` | the executable, `Bink2Scarlett.dll`, `XboxBvhBuilder.dll`, `amd_fidelityfx_upscaler_xbox.dll` |
| `tuxbox_xhttp.dll` | `build/xmem-smoke/` | `xmem.dll`, which forwards `XMemSetWinHttpHooks` to it |
| `d3d12_xs.dll` | `build/d3d12-smoke/` | the executable, `XboxBvhBuilder.dll`, `amd_fidelityfx_upscaler_xbox.dll` |
| `xg_xs.dll` | `build/xg-provider/` | the executable |
| `xapu.dll`, `chad.dll` | `build/xapu/` | the executable |
| `AcpHal.dll` | `build/acphal/` | the executable |
| `xcrdapi.dll` | `build/xcrdapi/` | the executable |
| `GameInput.dll` | `build/gameinput/` | the executable |
| `xgameruntime.dll` | `build/xgameruntime/` | the executable, at startup by name |

If using the application-local Windows compiler, copy both DLLs from the
same official release's `bin/x64` directory into the working copy beside
the game executable. Do not overwrite existing package files: compare them
first and use an untouched copy if they differ. The launcher warns about
missing or incompatible local DLLs but does not block a system-installed
compiler. The runtime reports an actual compiler load failure separately
from other pipeline errors. This is not a confirmed fix for the black-screen
report in #39.

Copy the providers again after each rebuild. The launcher enables the constant-buffer layer only when the staged `d3d12_xs.dll` is byte-identical to `build/d3d12-smoke/d3d12_xs.dll`.

## Launch offline

The game installs a Breakpad crash handler that uploads minidumps to an online crash service. Run it without network access so that crashes during testing are not uploaded. On Linux, start the launcher in a new network namespace:

```sh
UMU_RUNTIME_UPDATE=0 unshare --user --map-current-user --net -- \
    python tuxbox.py run --log-file build/scarlett.log /path/to/scarlett-run
```

Inside the namespace the only network device is a loopback interface that is down. `UMU_RUNTIME_UPDATE=0` stops umu-run from trying to update the Steam Runtime. Offline launches need unprivileged user namespaces.

Add `--mangohud` before the package path to record frame times. The prefix defaults to `~/.local/share/project-tuxbox/prefixes/Proton - Experimental`. Pass `--prefix` to use another one.

## Shader preparation

TuxBox records each shader the game requests in the prefix. A normal launch translates requests as they arrive and doesn't wait for the recorded set. Pass `--prepare-only` to compile every recorded request without starting the game, and the next launch reuses those translations. Place the option before the package path. [Preparation results](../evidence/projectscarlett-shader-preparation.json)

### Large pipeline compiles

The driver can take minutes and about a gigabyte of memory to compile one large
translated compute shader. When ProjectScarlett had sixteen in flight, it filled
its memory limit and swap and stopped progressing. TuxBox therefore lets at most
four translated compute shaders of 512 KiB or more compile at once; the others
wait. Smaller shaders are not affected.

Some of these compiles do not finish within any usable load time. A large
translated compute pipeline (device slot 62 or 86) therefore waits only
`TUXBOX_PIPELINE_DEADLINE` seconds (default 20). After that the game gets an
empty stand-in and the compile continues in the background. The stand-in does no
work, so whatever that program computes is missing until a later launch finds the
compiled result in the driver cache. `TUXBOX_PIPELINE_DEADLINE=0` waits instead. `TUXBOX_LARGE_COMPILES` sets the number,
and `0` removes the limit. The first launch can still spend a long time loading.

### GPU waits and sample masks

Two guest commands have no complete desktop equivalent. TuxBox logs each case
and keeps the queue running instead of failing it:

- A GPU memory wait recorded after rendering commands in the same list becomes a
  global barrier.
- A GPU memory wait that TuxBox performs on the queue is released after five
  seconds if its target value never arrives. The log line says
  `GPU memory wait released`. Work that depended on the missing write may then
  run too early.
- A sample mask that differs between the pixels of a quad is ignored, so the
  draw uses full coverage.

These keep a run going. They can also cause wrong output, so treat the log lines
as open compatibility problems.

### Finding why pipelines fail

Each run writes every pipeline that failed, or received an empty stand-in, to
`tuxbox_shader_cache/pipeline-failures/<run>.tsv` in the prefix's temporary
folder: how often it happened, the error, the creation path and the guest shader
digests. Rows also name each translation the pipeline looked up, as
`<stage>.used=<key>` or `<stage>.missing=<key>`. The key names the recorded
request, which differs from the shader digest when a root signature is part of
the request. To rank the reasons by the pipelines each cost:

```sh
python tools/rank_pipeline_failures.py "$PREFIX/drive_c/users/steamuser/AppData/Local/Temp/tuxbox_shader_cache"
```

The files stay on your machine. The printed ranking holds no paths or shader data.

The ranking uses explicit missing request keys when the run has lookup
telemetry, not raw shader digests from older compiler versions. A stage with
both successful and unresolved lookup attempts is ambiguous, not a proven
refusal. Stand-ins after successful translation and unlisted overflow rows are
reported separately from host failures. Refusal markers that disagree across
runtime cache versions are reported as conflicting; if you know which runtime
directory belongs to the run, pass `--runtime-cache /path/to/runtime/version`.
Older records without lookup telemetry still use the guest digests as hints.

### In-game progress panel

TuxBox draws a small panel over the presented frame while the game is translating
shaders or building pipelines. It shows how many translations are being waited
on and pipelines are being built, how many distinct shaders the game has received
in compiled form (and how many of those were translated during this run), how
many were unsupported or timed out, and how many pipelines were built, failed or
given an empty stand-in. It appears during that work, stays for four seconds
afterwards, and then hides. It is drawn by the runtime itself with GDI
and one D3D12 copy, so it needs no overlay software and works on Windows and
Linux. It never modifies the game's own image.

- `TUXBOX_PROGRESS_OVERLAY=always` keeps the panel on, `0` turns it off, and the
  default shows it only while that work is happening.
- `TUXBOX_PROGRESS_OVERLAY_POS` sets the corner: `tr` (default), `tl`, `bl` or `br`.

### Preparation speed and coverage

Launch-time preparation runs one compiler job per available core, up to 8. Set
`TUXBOX_SHADER_PREP_JOBS` to a value from 1 to 32 to change it; more jobs use
more memory.

A translator update starts a new runtime cache, but most shaders still translate
to the same HLSL. TuxBox keeps each validated DXIL under
`tuxbox_shader_cache/dxil`, keyed by the HLSL, the target profile and the DXC
and validator in use, and copies it instead of running DXC again. This applies
to preparation and to translation during play. A DXC or validator change starts
over. A shader whose optimised compile timed out goes straight to `-O0` after
that. Deleting the `dxil` folder is safe.

Set `TUXBOX_SCARLETT_SEED_PACK=1` to add every shader stored in the package's
`shaders.rpf` to the preparation list before compiling, so the first launch
does not depend on which scenes earlier runs reached. The launcher reads the
plain shader records after the archive's table of contents; it does not
decrypt anything. Linked vertex/pixel pairs, stencil variants and programs
that receive their root signature at pipeline creation are still recorded
during play. On the 1.0.2.1 `shaders.rpf` (412 MB) the reader found 25,889
distinct DXBC programs in 4.4 s; 5,527 of them are byte-identical to requests
the game recorded at run time.

### Live compilation workload

During gameplay, unrelated shader requests can compile concurrently rather
than waiting for one another. The launcher defaults to at most four jobs
(and no more than its available CPU cores); set
`TUXBOX_RUNTIME_SHADER_JOBS=1` for the old serial behavior or a value from 1
to 8 to tune the CPU/memory cost. Identical shader requests within the game process still share one
translation, including retries while the previous request remains published. Persistent refusal markers are only written for deterministic
emitter refusals, not transient tool errors. This reduces queueing delays but
does not make unsupported shader instructions compatible.

Synthetic compute benchmark (not game performance): generate a fresh corpus with
`python tools/reference/scarlett_worker_fixture.py /path/to/new-corpus`.
Use `tools/benchmark_scarlett_worker.py --help` for a paired comparison of
the old and new worker, keeping the compiler executables and corpus identical.
On the measured Ryzen 5 5600X, 64 cold requests took a median 0.881 s serial
and 0.300 s with four jobs (five rounds per configuration); eight jobs took
0.229 s but had higher peak aggregate RSS. All outputs matched by SHA-256.
A forced host-side warm-cache burst took 0.074 s serial and 0.096 s with four
jobs; a single cold request remained about 0.065 s. The actual guest normally
reads verified warm-cache entries directly. These are small synthetic compute
controls, not a prediction of GTA VI loading times.

The live guest control `tools/check_live_shader_worker.py` uses the synthetic
corpus above and a disposable offline Proton prefix. It makes 24 simultaneous
Xbox compute-pipeline calls for 16 distinct shaders, with null-pipeline fallback
disabled. On the RTX 4080, all 24 calls succeeded at both four and eight jobs;
each distinct source was compiled once in each run and the validated outputs
matched across configurations. See `--help` for reproduction arguments and
[the receipt](../evidence/scarlett-live-worker-control.json).



### Experimental persistent DXC

On Linux, build the helper against the same Microsoft DXC release used by
TuxBox. Set `DXC_ROOT` to the extracted release directory.

```sh
mkdir -p build
c++ -std=c++17 -O2 -isystem "$DXC_ROOT/include" tools/dxc_worker.cpp \
    -L "$DXC_ROOT/lib" -ldxcompiler -o build/dxc-worker
export TUXBOX_PERSISTENT_DXC="$PWD/build/dxc-worker"
```

The live worker starts helpers on demand for optimized `cs_6_0` requests.
Each helper handles one request at a time and stays open for the session.
The worker retries once if a helper exits during compilation and falls
back to the CLI if the helper or validation fails. Each newly compiled
result still passes DXV before publication. The CLI handles other profiles
and slow requests. The experimental cache identity includes the helper
binary. Unset `TUXBOX_PERSISTENT_DXC` to use the default CLI.

For a synthetic comparison, run the worker benchmark described above
with `--jobs 4` and add `--dxc-helper build/dxc-worker` for the experimental
variant. Give each run a fresh output path. Compare per-request SHA-256
values and latency.

## Prepare shaders after an update

A normal launch translates shaders as the game requests them. When an update
changes the shader translator, prepare the recorded requests first. Set
`TUXBOX_SHADER_PREP_JOBS` to run more translations at once.

```sh
python tuxbox.py run --prepare-only --backend proton --prefix PREFIX /path/to/scarlett-run/game_scarlett_master.exe
```

## What to expect

On November 25, 2026, an offline run on a GeForce RTX 4070 Ti SUPER got into the
story intro. The game's own crash report records the state `Game Running`, frame
32,534 and the story intro script after 18 minutes 11 seconds. GPU queues stalled
on fence waits there. The run ended on a game assert (exit code 3) after a command
list ran out of descriptor memory. The prologue cutscene did not appear.
[Run record](../evidence/projectscarlett-gameplay.json)

- **Fence stalls.** TuxBox now records the game's fence value writes, keeps a
  list's other work when one command can't be recorded, and no longer makes a
  queue wait on work that sits behind a label wait. These are checked by the
  [Lanes self-test](../examples/lanes/README.md#self-test). Stalls still occur at
  the intro. A wait that never completes is released after five seconds, and the
  log reports which lists write the label and where each queue stopped.
- **Descriptor memory.** Shader-visible descriptor slots are not reclaimed and can
  run out.
- **Missing effects.** Many shaders are still unsupported, and large compute
  pipelines can receive empty stand-ins. Expect missing or wrong rendering.
  Image correctness hasn't been checked. [Shader coverage](SHADERS.md#projectscarlett-refusals)
- **Long loads.** The driver takes minutes, and sometimes most of an hour, to
  compile the largest translated compute shaders.
- **Memory.** A long run needs about 15 GB of RAM plus swap. On a machine whose
  `/tmp` is held in memory, clear large temporary files before launching.
- **Not yet verified:** input and audio during play, frame times, and any other
  GPU or Windows.

An earlier run record covers startup up to the player selection prompt at
3840×2160. [Startup record](../evidence/projectscarlett-startup.json)

While it runs, the launcher maps the prefix's `G:` drive to the package, because
the game reads its files through that drive on the console. The previous links
are restored afterwards.

When you report a run, include the package version, executable hash, GPU and driver, Proton build, the log printed in the first line of the launcher's output, and the pipeline failure file described [above](#finding-why-pipelines-fail).


### Packed integer compute controls

The ordinary VOP3P path supports five additional forms:
`v_pk_add_i16`, `v_pk_mad_i16`, `v_pk_mad_u16`,
`v_pk_max_u16` and `v_pk_min_u16`. The low/high selectors are
independent, including the third operand's high selector in bit 14 of
the first word. Signed and unsigned arithmetic without clamp is modulo
2^16 per half. These new forms require ordinary VGPR inputs; negation and other
sources remain refused. Saturating arithmetic is described below.

[Validation receipt](../evidence/scarlett-packed-integers.json):
22 synthetic compute programs (all refused on the prior emitter),
176 selector combinations, with a source aliasing the destination,
16 independent dynamic inputs per combination, 2,816 calculated
results and 8,448 total readback words including input/guard values.
All matched the independent Python model on RTX 4080 and stock
Proton Experimental. Ten negative controls were refused in the original wrapping-only revision. This is
not a measurement of ProjectScarlett's actual shader coverage.

Build the test emitter with
`cargo build --manifest-path tools/shader-compiler/Cargo.toml --example packed_integer --target x86_64-unknown-linux-gnu`.
Use `python tools/check_packed_integer_gpu.py --help` for the local
LLVM gfx1030 assembler, emitter, DXC/DXV, provider and offline Proton
template arguments. An optional baseline binary built with the same
example and the original integer emitter enables the before/after
gate. No game files are used.


The follow-up also accepts CLAMP for seven packed add/subtract/multiply/MAD
forms with ordinary VGPR inputs. Each signed half clamps to -32768..32767;
unsigned halves clamp to 0..65535. Unsigned subtraction clamps before
underflow. Other modifiers, min/max/shift clamp, and special sources
are not broadened. AMD RDNA2 ISA sections 6.5 and 12.10 define these
integer limits and arithmetic operations.

[Follow-up receipt](../evidence/scarlett-packed-integer-clamp.json):
26 additional synthetic programs refused by the previous PR66 revision
now pass (208 selector combinations, 3,328 calculated results, 9,984
guarded readback words). The 22 original wrapping programs were rerun
and all 8,448 readback words still match. Seven new scalar-source clamp
controls remain refused. Run the same GPU control with `--clamp` to
exercise the added forms, and use the previous PR66 emitter as the
baseline. No real title or AMD hardware validation.


### Shareable preparation diagnostics

A completed preparation prints one `ProjectScarlett preparation aggregate:`
JSON line to stderr with checked/compiled/cached/unsupported/failed counts,
request types, fixed refusal buckets and failure buckets. This does not
change the compiler, marker format, timeouts or cache behaviour.
Each refusal marker has a `<key>.reason` file beside it holding the
emitter's one-line reason, so a warm run reports the same bucket as the
run that refused the shader. Markers written by older TuxBox builds have
no reason file and are counted as `cached_reason_unavailable`, not
guessed. Unknown reasons go into `other_refusal`. The aggregate line
alone contains no paths, request keys, shader data or raw diagnostics;
the surrounding normal output may contain private paths and should not be
shared without review.

For a full fresh emitter-only ranking even on a warm cache, use the existing
triage tool in aggregate mode against the authorized local request archive:

```sh
python tools/triage_scarlett_shaders.py "$REQUESTS" --jobs 2 \
  --aggregate-only --json scarlett-aggregate.json
```

Here REQUESTS is the recorded requests directory in the title-specific cache.
Share only the aggregate JSON (and tested commit/tool version separately).
The existing default detailed triage is still useful privately. Aggregate
mode uses fixed buckets; unfamiliar messages are deliberately grouped rather
than publishing arbitrary stderr. Emitter-only success does not imply DXC,
DXV, driver pipeline creation or gameplay success.

Motivation: [the current-main runtime log in issue #39](https://github.com/brcly/TuxBox/issues/39#issuecomment-7136369501)
(test commit 3086e0b, log SHA256
`9d728e2e9d8374702a094e973be03f6528d6c63e906e416594df1188543a5b34`)
contains 88 distinct explicit live refusals: 57 divergent wave reductions,
13 EXEC elections, 8 DPP prologues, 6 saveexec prologues, 2 DS swizzles,
one uninitialized lane read and one dynamic scalar offset. Its separate
preparation run reports 3,966 unsupported out of 8,692 requests. These are
different observation populations, not a denominator for the 88 live
refusals. The runtime log also reports 32 sampled slot-86 stage failures
(31 E_INVALIDARG, one E_FAIL, all unlinked), 131 host-memory import failure
messages and three signaller timeouts; none alone establishes root cause.
The highest remaining request-family ranking needs a fresh local triage,
not extrapolation from the 88 visible runtime refusals.
### CPU differential verification of integer halves

The independent packed-integer Python model now covers 14 wrapping packed
forms, seven saturating forms and ten single-half forms (31 variants).
The latter preserve the other half of the destination. Three additional
ordinary compute forms are supported: `v_mul_lo_u16`, `v_max_u16`,
`v_min_u16`. Their old destination is recorded as an input for the
partial write. New forms refuse non-compute stages, non-VGPR sources
and unsupported modifiers.

The CPU harness generates boundary and random inputs, all half selectors,
source/destination aliases and mixed dependent chains. LLVM gfx1030
assembles the synthetic native instructions, the full translator emits
HLSL, and DXC/DXV constant-fold/validate it. Only the final six observable
register stores are added by the harness. Missing, extra, duplicate or
nonconstant stores are failures, not successful comparisons.

[CPU receipt](../evidence/scarlett-packed-integer-cpu.json): before the
three additions 3,379/4,480 programs passed and 1,101 were refused; after
all 4,480 pass, with 26,880 observed values. A second seed with 4,096
mixed chains passes another 6,528 programs (39,168 values). No numerical
mismatches. A deliberate wrapping-add-to-subtract mutation is detected
in 30/608 cases and then removed. Six new negative controls still refuse
scalar operands and clamp. 264 unit tests pass (including latest main).

Build the `packed_integer` example as above and run
`python tools/check_packed_integer_cpu.py --help` for the LLVM, emitter,
DXC/DXV and fresh work/output arguments. Defaults use two CPU workers,
four input samples per selector pair and 2,048 mixed chains. No Wine,
Proton, game or GPU is required. This is not a replacement for subsequent
dynamic GPU and title-level validation.

## Memory and performance regression checks

The offline analysis tools below use CPU only. They do not start a game,
touch its prefix, probe the GPU, or change host settings. The memory sampler
reads an explicitly selected Linux PID; it does not discover other processes.

### Process memory

```sh
python tools/scarlett_memory.py capture <game-pid> --seconds 60 --interval 1 \
    --output memory.jsonl
python tools/scarlett_memory.py analyze memory.jsonl --json memory-report.json
```

The sampler checks the process start ticks before and after reading
`/proc/<pid>/smaps_rollup`, to avoid mixing a reused PID with the original.
It reports RSS, PSS (shared resident memory apportioned to this process),
private resident memory, swap and apportioned swap. Growth is evaluated
using PSS plus SwapPss, so swapping memory out does not appear to free it.
Reports include first/last-quarter medians, peak, peak time and retained
growth above the selected `--growth-kib` threshold (default 16 MiB).
A retained-growth flag requires at least 12 samples, the minimum capture
duration (default 30 s), no gap over three nominal intervals, and a
completed duration record. It is not proof of a leak: repeat the same
workload and allow caches to settle. Missing counters or access permission
are errors, not zero-valued memory. This does not measure process descendants,
GPU/driver memory, allocation ownership or inaccessible mappings.

Compare equivalent, complete captures with the same interval and similar
duration. Choose an allowed peak increase in KiB before testing:

```sh
python tools/scarlett_memory.py analyze candidate.jsonl --baseline baseline.jsonl \
    --peak-budget-kib 16384 --json memory-comparison.json
```

The optional allocation trace proposed in #51 can also be analyzed when
available. Use `TUXBOX_TRACE_ALLOCATION_BYTES=8192,16384,invalid` on a
build that includes that trace, then:

```sh
python tools/scarlett_memory.py allocations scarlett.log --json allocations.json
```

It joins interleaved header/item records by ID within each launcher session,
retains caller RVAs and resource descriptions, marks incomplete or conflicting
records, and flags the UINT64_MAX invalid-size sentinel. A capped trace
is incomplete evidence. These records are allocation *size queries*, not
allocations or frees: do not sum their returned bytes to estimate live memory.
The tool has no dependency on #51 and can report no observed queries.

### Repeatable frame-time regression gate

Use existing `--mangohud`, `--profile-pipelines` and
`tools/sample_process_cpu.py` captures as inputs, without altering the
existing launcher summary. Keep hardware, driver, backend, scene, settings,
resolution and cache state identical across the compared builds; alternate
baseline/candidate runs and record the actual process lifetime for each.
Create a JSON manifest (paths relative to the manifest, or absolute):

```json
{
  "schema": 1,
  "context": {
    "scenario": "fixed test scene and capture window",
    "hardware": "same machine and GPU",
    "driver": "same driver version",
    "backend": "same Proton version",
    "resolution": "1920x1080",
    "settings": "same graphics profile",
    "cache_state": "warm"
  },
  "budgets": {
    "mean_fps_drop_pct": 5,
    "p95_increase_pct": 10,
    "p99_increase_pct": 10,
    "one_percent_low_drop_pct": 10,
    "stutter_fraction_increase": 0.01
  },
  "stutter_ms": 50,
  "min_frames": 300,
  "max_skipped_fraction": 0.05,
  "pairs": [
    {"baseline": {"frames": "old1.csv", "seconds": 60},
     "candidate": {"frames": "new1.csv", "seconds": 60}},
    {"baseline": {"frames": "old2.csv", "seconds": 60},
     "candidate": {"frames": "new2.csv", "seconds": 60}},
    {"baseline": {"frames": "old3.csv", "seconds": 60},
     "candidate": {"frames": "new3.csv", "seconds": 60}}
  ]
}
```

```sh
python tools/compare_scarlett_performance.py runs.json --json performance-comparison.json
```

The tool reuses the launcher's frame-time parser, filters invalid and
uptime-like samples, rejects insufficient evidence, and calculates mean FPS
from total valid frame duration, nearest-rank percentiles, 1% low FPS
(inverse mean of the slowest ceil(1%) frames), and the fraction of frames
strictly above the chosen stutter threshold. Every individual pair must
stay within the declared budgets; a favorable median cannot hide a bad
run. Positive reported changes represent regressions. The stutter fraction
budget is an absolute fraction (0.01 is one percentage point).
Three to thirty independent pairs are required. Declared conditions are
not automatically verified; choose equivalent capture windows. A frame
time is not a measurement of game simulation progress.

Each paired run can optionally include a `"cpu": "cpu.jsonl"` and/or
`"pipelines": "timeline.json"` path. CPU usage is weighted by measured
interval duration, in percent of one core, with observed thread-name
totals. Pipeline timings use the existing vkd3d trace summarizer. They
are summed call durations (can overlap), not elapsed wall time or proof
of the bottleneck. These sidecars are diagnostic, not performance gates.
Compare tools return 1 for a violated budget and 2 for invalid input.
No GPU tests or retail frame captures are included in this contribution.

Reproduce real 32 MiB reservation/release in a disposable Linux process
and six synthetic frame-time pairs:

```sh
python tools/reference/scarlett_resource_fixture.py /tmp/resource-control
```

[Validation receipt](../evidence/scarlett-resource-analysis.json) records
the sampled memory and the improved/negative frame-time controls. In the
negative control two pairs improve but the third contains 100 ms stalls;
the all-pairs gate correctly fails despite the improved median.

### CPU scalar reference and generated semantic tests

The independent reference model (`tools/reference/rdna2_scalar.py`) decodes
native words and models ordinary integer SGPR operations using the
[AMD RDNA 2 ISA reference](https://www.amd.com/content/dam/amd/en/documents/radeon-tech-docs/instruction-set-architectures/rdna2-shader-instruction-set-architecture.pdf)
(SOP1, SOP2 and SOPC). It is intentionally not a complete Xbox GPU simulator.
The model covers 77 instruction forms, ordinary registers s2..s9, SCC,
32/64-bit arithmetic, bit operations, shifts, comparisons and conditional
moves. It rejects unsupported selectors, incomplete programs, uninitialized
reads, and unsupported instruction classes instead of inventing results.
It does not simulate memory, textures, wave execution, vector lanes,
floating point, control flow or hardware timing.

Generate deterministic boundary-value and random programs, including
overlapping registers and chains. The core profile covers 65 forms accepted
by the baseline; extended adds the 12 forms proposed in #58.

```sh
python tools/check_scalar_encodings.py --llvm-mc /path/to/llvm-mc
python tools/generate_scalar_tests.py /tmp/scalar-corpus --per-op 128 --chains 256
cargo test --manifest-path tools/shader-compiler/Cargo.toml \
    --target x86_64-unknown-linux-gnu --lib --no-run
```

Use the Rust library test executable printed by Cargo (the
`tuxbox_xshader-...` binary, not the CLI executable):

```sh
python tools/check_scalar_oracle.py --corpus /tmp/scalar-corpus \
    --bridge /path/to/tuxbox_xshader-test-binary \
    --dxc /path/to/dxc --dxv /path/to/dxv \
    --work /tmp/scalar-check --output /tmp/scalar-receipt.json --jobs 2
```

The bridge runs the production native decoder and full translator. It batches
complete emitted HLSL function bodies only when their helper headers match,
and adds stores for all eight registers and the condition code. All input
registers and SCC are initialized by each synthetic program. DXC folds the
constant computations; DXV validates the resulting DXIL. Missing, duplicate,
nonconstant or mismatched stores fail the gate, as do translation refusals or
tool failures. The Python checker recomputes expected values independently
from the native words, not from the translator's HLSL or a trusted manifest.
The test does not execute generated DXIL on hardware.

Inspect any case step-by-step without invoking a compiler:

```sh
python tools/reference/rdna2_scalar.py /tmp/scalar-corpus/programs.txt --case 000000
```

Results on the measured CPU-only campaign:
- LLVM confirms all 77 instruction encodings.
- Core: 8,577/8,577 programs, 77,193 register/flag values, no mismatches;
  also passes on the separate #58 translator test build.
- Extended on #58: 10,026/10,113 programs, 90,234 compared values, no
  numerical mismatches. 87 chained programs are conservatively refused,
  and the gate correctly fails. These are reproducible remaining limitations,
  not confirmed failures in the retail game.
- A deliberately faulty addition-to-subtraction translator in a temporary
  test worktree produced five mismatches. The mutation is not in this PR.

See [the receipt](../evidence/scarlett-scalar-oracle.json) for case IDs,
seeds, exact corpus/compiler digests and counts. This is a reusable test tool,
not a change to the shipping translator or a claim of retail compatibility.

Rechecked the same core corpus after updating to upstream `3086e0b`: all
8,577 cases passed again. The earlier #58 and mutation receipts retain their
original source fingerprints.


### Bounded differential campaign and small reproducers

Use the existing CPU reference and full-translator bridge; no new runtime
dependencies. The campaign generates initialized straight-line scalar
instruction chains, verifies every distinct native instruction encoding
against LLVM gfx1030, checks the same programs with the #60 oracle and
DXC/DXV, and groups refusals by exact text with the instruction offset
removed (the same offset normalization principle as #44). Mismatches are
grouped by differing output indices, not claimed identical root causes.
A representative of each selected group is rechecked individually, reduced
by removing complete instructions after the fixed nine-instruction
initialization, then replayed twice in fresh compiler calls. Local
one-deletion minimality is recorded; this is not a global minimum. The
report stores the native words, assembly, expected state, case digest,
classification and counts, as well as structural opcode/alias/adjacency
features (not compiler branch coverage). Tool errors are not cached or
reported as confirmed shader defects.

```sh
python tools/scarlett_campaign.py --bridge /path/to/test-binary \
    --dxc /path/to/dxc --dxv /path/to/dxv --llvm-mc /path/to/llvm-mc \
    --profile extended --count 2048 --reduce 12 \
    --work /tmp/scalar-campaign --output /tmp/scalar-campaign.json
```

The default core profile is for main without #58. Extended requires the
proposed #58 operations for a passing run. Use a wrapper executable for
LLVM if it needs a container. Reuse a confirmed minimized corpus with
`--replay-report evidence/scarlett-differential-campaign.json --require-pass`
instead of generating new cases (use fresh work and output paths).
Replay deliberately bypasses cache. The normal discovery mode reports
refusals without failing the command; `--require-pass` also fails for them.
The campaign does not establish actual title coverage.

Optional trusted local `--cache /path/to/cache.json` and repeated
`--dependency /path/to/compiler-library.so` arguments retain exact
content-keyed results for repeat work. Include all actual loaded compiler
libraries (e.g. identify via ldd on trusted local binaries); a different
tool, source, dependency, Python version or environment refuses reuse.
Use one owner per cache, not concurrently shared or externally supplied
cache files. Cache is an optimization, not an independent proof; use a
fresh cache or no cache for release/regression verification. The tool
limits compiler workers to two, generated cases to 8,192, total evaluated
cases/cache entries to 20,000, reductions per representative to 64 and
representatives to 32. Work is temporary; per-tool timeouts apply and no
new evaluation starts beyond the 240-second campaign budget.

[Receipt](../evidence/scarlett-differential-campaign.json): 2,048 core
programs pass; 2,048 extended programs on the original #58 expose 483
refusals across twelve known provenance-related instruction signatures,
all confirmed as small reproducible cases. With the conservative compute
provenance follow-up all 2,048 pass, and the older 8,577 core plus 10,113
extended cases pass. The fix remains a separate change to #58. No accepted
case shows a numerical disagreement. Five alternating fresh/reused
evaluations of 512 identical programs (seed 17294091; LLVM checks in
both arms; cache primed once) measured 4.63 s vs 0.47 s median on the
local machine. This is repeat validation with an unchanged toolchain,
not faster cold discovery or a claim of higher FPS. The campaign remains
deliberately narrow: no memory, wave execution, branches or GPU testing.

## Offline launch and visual diagnostics

These tools consume existing logs or captures; neither starts the title,
touches the Wine prefix, changes driver settings, or uses a GPU.

### Locate a launch failure

```sh
python tools/diagnose_scarlett_run.py build/scarlett.log --json build/diagnosis.json
```

The diagnostic reads the launcher's actual status format, including appended
sessions, recorded D3D12 milestones, shader compiler outcomes, explicit
pipeline/resource/command failures, missing imports, survey call-site RVAs
and unhandled Wine exceptions. It reports line numbers and timestamps
where available, counts diagnostic events separately from distinct shader
requests, and suggests the next check for each observed fault type. Terminal
exception/device-loss evidence is listed first, then recent diagnostic
groups by line number; this is an investigation order, not causal ranking.
An absolute fault address is not a title RVA. Survey asserts that were
continued are not terminal errors. A missing exit record remains incomplete,
not a diagnosed hang. The first presentation does not prove visible output.
The input file should be complete and stable; mixed process output cannot
establish exact timing or causal attribution. Files over 512 MiB are rejected
and lines over 64 KiB are counted and skipped. Reports omit raw log messages
and paths, but may include module names and addresses; review before sharing.

In the public issue #39 log, the tool identifies the first presentation at
line 514, page mapping at 548, the slot 86 module-not-found HRESULT at 541,
64 logged compute-pipeline failure events and 836 distinct refused shaders.
The log has no exit record; the missing module is not identified by its
HRESULT, and these observations do not establish the ultimate stop.

### Compare rendered frames

Use a Python environment with Pillow installed (the existing project
requirements are unchanged; the frame analysis is optional).
Capture reference and candidate at the same camera, resolution, frame,
UI state and color settings, preferably lossless RGB or RGBA 8-bit SDR PNG.
The tool cannot automatically establish this equivalence.

```sh
python tools/compare_scarlett_frames.py reference.png candidate.png \
    --json frame-comparison.json --heatmaps frame-errors
```

For a sequence, pass two directories of equally named PNGs; names must
match exactly. The tool compares in lexicographic name order and counts
adjacent identical pixel digests, not elapsed time or proven freezes.
Dimensions, color metadata and modes must match. No automatic alignment,
resizing or color conversion. RGBA compares all four straight channels,
including transparent RGB. Optional `--roi X0 Y0 X1 Y1` uses half-open
coordinates to exclude deliberately chosen UI/overlay areas.

Default gates require exact agreement. Set `--tolerance` to a maximum
per-channel integer difference and both `--max-fraction` and
`--max-tile-fraction` to the allowed fraction of pixels exceeding it,
globally and in each tile. Choose thresholds before testing; the local
gate protects small defects from being hidden by global averages.
The report includes MAE, MSE, PSNR (null for exact equality), bounding box,
worst tiles, pixel digests, near-black/flat-color observations and optional
absolute-error grayscale PNGs (0=no difference, 255=maximum difference).
It returns 1 for a failed comparison, 2 for invalid input. Output paths
must be fresh. Maximum capture size is 16,777,216 pixels and maximum
sequence length 256 pairs. HDR/16-bit captures, animation, perceptual
quality and gameplay assessment are out of scope.

Reproduce six synthetic 4K comparisons and two independent launch sessions:

```sh
python tools/reference/scarlett_observability_fixture.py /tmp/observability-control
```

[Validation receipt](../evidence/scarlett-observability.json) includes the
public log fingerprint, controlled diagnostic outcomes and six 4K pairs
(identical, one changed pixel, local defect, black, tolerated one-level
changes, and swapped color channels). These are CPU-side checks, not real
title rendering comparisons.


The offline diagnosis also counts driver allocation attempts by memory type
and Vulkan result, host-memory import failures, shader interface mismatch
messages, driver compiler failures, and signaller timeouts. It tracks the
maximum reported cumulative null-pipeline and cache-skip counters rather than
summing their samples, and reports sampled stage-pair counts and link status
without disclosing shader identifiers. These are observed log events, not
independent allocations or confirmed causes. A missing exit record remains
an incomplete log, not proof of a crash or hang.

Applied to the public issue #39 retest log (SHA256
`9d728e2e9d8374702a094e973be03f6528d6c63e906e416594df1188543a5b34`),
this distinguishes 262 failed memory-allocation attempts from 131 host-memory
import failures, 12 shader interface mismatch messages, 32 sampled unlinked
stage failures and three signaller timeouts. The last recorded draw does not
establish a visible frame or gameplay. The recorded maximum counters are
1,024 compute and 2,048 raster-stream substitute pipelines; not exact final
totals or distinct shaders. No retail inputs were used locally.
### Ordinary scalar bit operations

The generic translator supports nine additional scalar bit instructions and applies the ordinary-scalar proof to the three forms already added by #56
(32/64-bit zero counts, zero searches, leading-bit searches, reversal,
bit setting/clearing and testing). The local reaching-definition check
requires ordinary scalar values, not an assumed full-wave EXEC/VCC mask.
Unknown halves, branch entries and indirect control flow remain unsupported;
private packed and mesh paths are outside this proof. The rules follow
[AMD's RDNA 2 ISA reference](https://www.amd.com/content/dam/amd/en/documents/radeon-tech-docs/instruction-set-architectures/rdna2-shader-instruction-set-architecture.pdf), SOP1/SOPC.

CPU-only controls:

- Capture `cargo test --manifest-path tools/shader-compiler/Cargo.toml --target x86_64-unknown-linux-gnu scalar_bit_cpu_oracle_fixture -- --nocapture`
  to a fresh fixture file. Run `python tools/check_scalar_bits.py --help`
  for the DXC/DXV numerical oracle: 1,944 cases, 7,776 checked values.
- Run `python tools/check_scalar_native.py --help` with the baseline and
  candidate xshader executables: 13 synthetic native programs recovered,
  six deliberately unsupported controls retained, one unchanged control.

[CPU evidence](../evidence/scarlett-ordinary-scalar-bits.json). These controls
are not retail shader coverage or proof of GPU execution.


The dependent-compute follow-up preserves ordinary scalar provenance through
verified min/max, wide shifts, bit updates and conditional/carry operations
only when the ordinary inputs (and, when required, SCC) are proven. It
does not relax branch, special wave-mask, mesh or unknown-input checks.
The CPU campaign found 483 rejected programs among 2,048 synthetic extended
programs on the previous #58 implementation, across twelve known
provenance-related refusal signatures. The twelve minimized native
reproducers are now permanent compiler tests. The full 2,048-program
campaign and the older 8,577 core plus 10,113 extended CPU-oracle programs
pass on the integrated current main + follow-up build (186,642 observed
register/flag values in total). Negative controls reject unknown SCC,
EXEC-derived SCC and unproven high halves. See
[evidence](../evidence/scarlett-dependent-scalar-chains.json). No GPU or
retail compatibility claim.

### Investigate allocation-size mismatches

For a diagnostic launch, set `TUXBOX_LOG=stderr` and
`TUXBOX_TRACE_ALLOCATION_BYTES=8192,16384,invalid` before the normal offline
launch command. Rebuild and restage the provider first. The optional trace
reports exact selected allocation sizes (decimal or 0x-prefixed hex), including
the failing size sentinel with `invalid`. It records the title-relative caller,
resource descriptions and returned alignment, not resource contents. Repeated
records are deduplicated; at most 256 distinct records and eight descriptions
per record are emitted. Lines share an ID to correlate interleaved threads.
No allocation decisions are changed. This is intended to help locate the
source of a size mismatch, not to bypass the title's assertion.


For host heap import failures, set `TUXBOX_TRACE_HEAP_IMPORTS=1` and
`TUXBOX_LOG=stderr` after rebuilding/restaging the provider. This optional
trace identifies the failing native heap-open API (sized or legacy), the
provider caller path (virtual placement, layout placement or view), requested
bytes, HRESULT and title-relative caller. It omits host addresses and
resource data, deduplicates the first 256 distinct failure keys, and does
not change allocation, lifetime, fallback or error-return behavior. It
does not trace imports performed internally by the desktop driver.
The issue #39 log contains 262 failed driver allocation attempts and
131 host import failure messages, not a confirmed VRAM exhaustion or
a proven fault in these provider call sites. The opt-in trace is intended
to distinguish those possibilities in a subsequent authorized run.
