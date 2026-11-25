// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Host;

/// <summary>The usage text of fivestar-loader.</summary>
internal static class HostUsage
{
    public const string Text = """
        fivestar-loader - manual PE loader and Xbox API shims

          fivestar-loader --load <dll> [--load <dll> ...] [--call <export>|all] [options]

        options:
          --call <name>          call an inspected export (see --list-safe-calls); 'all' = every applicable one
          --run                  run the main executable's entry point; reports faults in mapped code with the
                                 last calls through shims and stubs
          --run-timeout <s>      with --run: after s seconds report the main thread's stack and exit (code 3)
          --cpu <host|intel>     intel: report SSE4a, CLZERO, MONITORX/MWAITX and CLWB as missing and check their
                                 emulation through the exception handler (self-test code, not the game's)
          --trace-system         with --run: the call trace also records System32 imports
          --input-script <file>  timed key presses ("<s> <key> [hold <s>] [repeat <n> every <s>]", "<s> capture", "<s> learn <screen>" per line; "screen <name>" starts a block timed from when that screen shows; reloaded when the file changes)
          --screens <file>       learned screens of --input-script (default %LOCALAPPDATA%\FiveStar\screens.txt)
                                 added to the desktop keys, so the keyboard and the virtual gamepad see them
          --profile-threads      with --sample-threads: also log the main image's busiest functions on running threads
          --profile-managed <start>[:<seconds>]  from <start> s into the run sample managed stacks for <seconds> (20)
                                 into <log>.managed.nettrace; fivestar profile summary reads it
          --trace-module <dll>   with --run: the call trace also records calls into this mapped image
          --sample-threads <s>   with --run: every s seconds log the calls per shim since the last sample,
                                 DirectStorage totals and every thread's stack
          --memory-map <s>       with --run: every s seconds log the private memory by owner, protection and
                                 allocation size, its change, the largest allocations and GPU memory
          --min-free-memory <GB> with --run: end the process (code 5, after a last memory map) when the machine's
                                 free physical memory drops below this
          --max-commit <GB>      with --run: the same when the machine's commit charge passes this
          --section-heap-budget <GB>
                                 at most this much of the game's page sections backed on the GPU (default: what
                                 the GPU's system-memory budget leaves); the rest opens as it is used, or reads as zeros
          --in-place-window <MB> the largest heap opened in place over game memory (64 to 1024, a power of two; default
                                 1024): larger sections and the game's own allocations open in windows of it as used
          --evict-windows        prototype: section windows the address map needs open when programs report reading
                                 them and the least recently wanted are released for them (programs made on demand)
          --reserve-gpu-system-memory <GB>
                                 debugging: hold this much GPU system memory (upload buffers, RAM) from device creation,
                                 so its budget is as tight as on a machine with less RAM
          --main <name>          image that plays the process exe for GetModuleHandle(NULL)/GetModuleFileName(NULL)
                                 (default: first mapped exe, else the first --load)
          --patch-peb            also point PEB.ImageBaseAddress at the main image (off by default)
          --save-dir <dir>       XGameSave / local storage root (default: %LOCALAPPDATA%\FiveStar\GdkData\<TitleId>)
          --shader-dir <dir>     extracted FSSM programs: D3D12.X pipeline creation is enabled, programs are translated
                                 and compiled on first use unless --translated-dir has them
          --translated-dir <dir> translated DXIL programs (fivestar shader translate), optional
          --program-cache <dir>  programs made on first use (default: <work-dir>\program-cache)
          --no-on-demand         only --translated-dir provides programs
          --dxc <path>           dxcompiler.dll (or dxc.exe, or their folder) for programs made on first use;
                                 default: FIVESTAR_DXC, beside the loader, then the Windows SDK
          --shader-jobs <n>      threads that make programs on first use (default: half the processors with the
                                 DXIL backend, a quarter with dxc)
          --loop-caps <caps>     loop caps of programs made on demand: victory (4096 compute, 256 vertex/mesh, 128 pixel),
                                 default, or compute,geometry,pixel; part of the program cache identity
          --superstep-round-budget <k>
                                 a superstep program's group rounds at most k per wave-operation stop of its code (at
                                 least 256): a cut before the driver's timeout culls the draw; part of the cache identity
          --host-pipelines       pipelines of more than 2 MB of DXIL are built first in a child process held to the
                                 free memory and 10 minutes; one past a limit stays a stand-in (off by default)
          --plain-supersteps     superstep forms made on demand keep every register live around one dispatch
                                 switch (the form before the liveness carry); part of the cache identity
          --fast-waves           approximate: a mesh program that needs its whole console wave runs natively on
                                 narrower waves, each wave's first lane taken for its console wave's and any-lane branches decided per wave,
                                 instead of in the superstep form (not when it reads a lane by number); part of the
                                 cache identity
          --committed-textures   a committed resource for each read-only console texture instead of the shared
                                 placed heaps (the default, also --placed-textures), whose released ranges stay mapped
          --prebuild-jobs <n>    threads that build, from device creation on, the pipelines earlier boots used that the
                                 disk cache lacks, giving way to the game's compiles (default 0: off)
          --wait-pipelines <name,...>
                                 a stand-in pipeline whose name holds one of these parts is waited for (up to 60 s, the
                                 game's hang watchdog paused) when a command list first sets it, instead of skipping its work
          --typeless-targets     GPU-written R8G8B8A8/B8G8R8A8/B8G8R8X8/R8 UNORM textures made typeless, so a view in
                                 their sRGB or integer member reads as the game asked (off by default)
          --kept-aliases <n>     textures retired over exactly one reused range kept alive for older descriptors (default 1)
          --max-driver-compiles <n>  driver pipeline compiles of 256 KB of DXIL or more at once (default one per 5 GB of RAM, 2 to 12)
          --compile-ceiling <GB> no such compile starts while private bytes and the running ones' expected arenas
                                 would pass this (default none: free RAM less the game's growth decides); one
                                 always may when none runs
          --shader-backend <b>   dxil (default): programs made on first use compile with the DXIL backend, dxc
                                 for those it cannot lower or dxil.dll rejects; dxc: dxc for all
          --pipeline-cache <dir> disk cache of built pipelines (default: <work-dir>\pipeline-cache)
          --no-pipeline-cache    build every pipeline anew and store none
          --eager-pipelines      build every pipeline the cache lacks at creation; default: those earlier boots
                                 used (pipelines-used.bin) at creation, the others when first set
          --warm-pipelines <shaders.rpf|blob>  no game: build the pipelines earlier boots asked for (first-use order),
                                 then every pipeline of the console pipeline cache, into the disk cache (with
                                 --shader-dir, --translated-dir), resuming where a run stopped
          --warm-used-only       warm-up: only the pipelines earlier boots asked for
          --warm-jobs <n>        pipelines the warm-up builds at once (default 4; split over the processes)
          --warm-processes <n>   warm-up processes, each building every n-th pipeline into the same cache, pipelines of
                                 4 MiB of DXIL or more left to a last pass (default one per 16 GB of RAM, 1 to 4)
          --warm-step <k>        warm-up: every k-th pipeline only
          --warm-match <text>    warm-up: only pipelines with a program whose name contains text (repeatable)
          --gdk-abi <file>       XGameRuntime thunk ABI table: "clsid iid slot FunctionName" per line;
                                 default: read from an installed PC GDK's xgameruntime.lib
          --input <file>         GameInput keyboard-and-mouse gamepad mapping ("setting value" per line)
          --ffx <dll>            PC FidelityFX API DLL (AMD FidelityFX SDK, DX12) the game's FidelityFX calls go to;
                                 without it FidelityFX reports no provider
          --d3d12-debug          D3D12 debug layer and DRED; their messages and the removal report go to the log
                                 (needs the Graphics Tools optional feature)
          --d3d12-gpu-validation --d3d12-debug plus GPU-based validation (slow)
          --force-cull-none <text>  pipelines whose programs' names hold <text> cull no faces (a winding check)
          --skip-pipelines <text,...>  pipelines whose programs' names hold a text stay stand-ins: never built or
                                 loaded, their draws skipped (to get past one that hangs the GPU)
          --mirror-table-records reach|full|<n>  records every descriptor-table bind mirrors (default 64): full
                                 (1024) gives lighting its inputs but mesh programs then reach data they hang on;
                                 reach mirrors each draw's tables as far as its programs read
          --d3d12-dred           DRED alone (auto-breadcrumbs, page faults, the removal report), without the debug layer
          --d3d12-skip <i:n,...> D3D12.X slots that do nothing (list:51, queue:35, device:n), to bisect GPU faults
          --d3d12-trace-queues   log the first queue submissions, signals and waits (cross-queue protocol)
          --d3d12-breadcrumbs    a GPU marker after every draw and dispatch: a stalled queue names the one it hangs in
          --watch-transient      check the game's transient-memory pool and log the first corrupted node
          --watch-pool-pages     check the game's small-block pool pages for headers its allocator did not write
          --skip-compute-chains  skip the rest of a command list's dispatches after one skipped for a stand-in
          --empty-indirect-after-skip  a command list's ExecuteIndirects after a compute dispatch skipped for a stand-in run nothing
          --retire-waits-all-queues  a submission holding the game's retire write waits for every other queue
          --no-window-guard      programs' stores and atomics through 32-bit pointers reach the descriptor window (by
                                 default they are dropped and counted per program at each captured present)
          --buffer-records-heap-index  buffer descriptor records hold the desktop heap index where the console has the
                                 view's base address (the behaviour before; programs that build pointers from a V# break)
          --typed-buffer-views   buffer views at slots programs reach by heap index keep the game's typed or structured
                                 kind (the behaviour before); by default they are raw, as programs read them
          --check-null-addresses  root views, indirect argument and count buffers, bottom-of-pipe writes and buffer
                                 copies that would reach a null GPU address or read past their buffer are logged
                                 (the first 8 of each kind, with list and pipeline) and dropped
          --user-data-constants  draws pass their user data as root constants, not a root CBV in upload memory (needs
                                 programs made on demand)
          --trace-pool-stores    programs made on demand log their stores into the transient pool and the game heap
          --watch-hang           log the game's main state, loading-screen flags and heartbeat, and every thread's
                                 stack while the heartbeat its hang watchdog checks stands still
          --d3d12-trace-draws    log what the first draws bind (pipeline, targets, vertex buffers, tables) and read
                                 their render targets back
          --d3d12-trace-pipeline <text>  with the draw trace, also log the draws of pipelines named with <text> in
                                 the frame after each captured present
          --d3d12-trace-records <offset,...>  for those draws, the window records at these byte offsets from each
                                 user SGPR holding a window offset: dwords, the V#/T# base's map state, a V#'s first values
                                 (offset@byte: also the dword at that byte of the V#'s buffer, e.g. 0x380@0x11c)
          --probe-pipeline <text>  mesh programs named with <text> are made with probe code: per group its output
                                 counts, culling, vertex 0's position and the load sites that read out of range,
                                 unmapped pages or null records, summed up at each captured present (--capture-presents)
          --d3d12-dump-targets   the draw trace, and in the frame after each captured present every render and depth
                                 target read back when a list leaves it, as PNG in the capture folder
          --capture-dir <dir>    write read-back planes and render targets there as PNG
          --capture-presents <n,...> present numbers whose plane is read back (default 60,300,900,1500)
          --present-test         present moving color bars instead of the game's frame (checks the swap chain)
          --raytracing          XboxBvhBuilder.dll's CPU paths: console acceleration-structure sizes and BLAS
                                 rehydration into game memory (off: desktop sizes, nothing written)
          --raytracing-gpu       run XboxBvhBuilder.dll's constructor for an inventory of its 52 GPU builder pipelines:
                                 stand-ins never built, no builder program runs (docs/bvh-builder.md)
          --no-rt-stand-ins      held ray-tracing passes leave their outputs as they are (default without --raytracing:
                                 shadow masks fully lit, reflections black, the screen-probe light a grey)
          --xdsp-convolution     run the XDsp convolution effects on the CPU (default: silence; response format inferred)
          --audio-capture <wav>  record what the game plays (process loopback, 48 kHz stereo) and print how much had sound
          --desktop-timers       keep 15.6 ms timer ticks (default with --run: 1 ms, which the game's audio mixer needs)
          --bcpack-tables <file> the BCPack tables (bcpack-tables.bin, not shipped; fivestar setup imports them):
                                 every BCPack codec decodes exactly; without them BC3, BC6H and BC7 have no
                                 decoder and BC4 and BC5 decode approximately
          --bcpack-missing <m>   DirectStorage BCPack pages without exact or approximate blocks:
                                 placeholder (default: neutral blocks of the page's format) | fail (E_NOTIMPL)
          --package-root <dir>  package root for XPackage / MicrosoftGame.config (default: main image directory)
          --device-type <n>      XSystemGetDeviceType result (default 7 = XboxScarlettAnaconda)
          --network <mode>       offline (default: connectivity level None) | sealed-online (InternetAccess); either
                                 way every WinHTTP, Winsock and libHttpClient connection fails inside the process
          --network-level <l>    connectivity level reported instead: 0..4 or Unknown, None, LocalAccess,
                                 InternetAccess, ConstrainedInternetAccess
          --patches <path>       code patch file, or folder of *.fspatch files (repeatable, applied in order)
          --mods <dir>           mod folder laid over the package root (repeatable, first has priority)
          --manual <a.dll,b.dll> map these modules with this loader instead of LoadLibrary (repeatable)
          --system-crt           do not map the shipped MSVC runtime (default: msvcp140*/vcruntime140*/concrt140/
                                 vccorlib140/vcomp140 are mapped from the target's directory if present)
          --search <dir>         extra directory for --manual modules and for modules the PC does not have
          --unresolved <mode>    stub (log + return 0, default) | trap (log + terminate on call) | error (fail load)
          --no-entry             do not call DllMain
          --no-shim <mod!func>   disable one shim (exercise the unresolved-import path)
          --no-exports           do not list the target's exports
          --unload               call DllMain(PROCESS_DETACH) and free images at the end
          --work-dir <dir>       where generated TLS host DLLs go (default: <exe dir>\work)
          --log <file>           full log (every import and its resolution)
          -v                     verbose console (every import, every shim call)
          --list-safe-calls      list the export call recipes
          --classify <file>      classify "module!symbol" lines (shim, Xbox stub, package, host or missing; the
                                 --search directories are the package folder; for fivestar exe analyze)
          --classify-out <file>  where the classification goes (default: standard output)
        """;
}
