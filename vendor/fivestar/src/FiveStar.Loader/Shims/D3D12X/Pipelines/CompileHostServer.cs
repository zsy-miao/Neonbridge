// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Graphics.Programs;
using FiveStar.Loader.Diagnostics;
using FiveStar.Shaders.Compilation;

namespace FiveStar.Loader.Shims.D3D12X.Pipelines;

/// <summary>
/// <c>fivestar-loader --compile-host [dxil.dll]</c>: serves the loader's requests, each on a thread of its own at the
/// asking thread's priority, and answers in the order they finish. A compile runs the DXIL backend
/// (<see cref="InProcessBackend"/>); a make also translates the program and picks the adapter's form first
/// (<see cref="ProgramProducer.Prepare"/>), so the translator's allocations stay out of the loader as well. It ends when
/// its input ends, which is when the loader ends.
/// </summary>
internal static class CompileHostServer
{
    private const int Success = 0;
    private const int ValidatorArgument = 1;
    private const string DxilExtension = ".dxil";
    private const long IdleHeapBytes = 4L << 30;
    private const long BusyHeapBytes = 6L << 30;
    private const long CollectIntervalMilliseconds = 10_000;

    private static long _lastCollect;

    private static int _active;

    /// <summary>Three compiles of a source of <see cref="ProgramForm.HeavySourceLength"/> or more at a time: 16 at once took the host to 8 to 12 GB (offline, 33 such programs on 16 workers: 8.4 GB; with three at a time 3.0 GB at 1.7 instead of 2.8 programs/s).</summary>
    private static readonly SemaphoreSlim Heavy = new(3);

    private static readonly System.Collections.Concurrent.ConcurrentDictionary<(string Programs, string Folder, FiveStar.Graphics.AdapterInfo Adapter, string Header), ProgramProducer> Producers = new();

    public static int Run(string[] args)
    {
        string? validator = null;

        if (args.Length > ValidatorArgument)
        {
            validator = args[ValidatorArgument];
        }

        var backend = new InProcessBackend(validator);
        var warnings = new OnceLog(new LoaderLog(TextWriter.Null, null, false));
        using var input = new BinaryReader(Console.OpenStandardInput());
        using var output = new BinaryWriter(Console.OpenStandardOutput());
        var gate = new Lock();

        while (true)
        {
            int id;
            ThreadPriority priority;
            CompileRequest? compile;
            MakeRequest? make;

            try
            {
                (id, _, priority, compile, make) = CompileHostProtocol.ReadRequest(input);
            }
            catch (EndOfStreamException)
            {
                return Success;
            }

            Interlocked.Increment(ref _active);
            var thread = new Thread(() =>
            {
                var answer = Serve(backend, warnings, compile, make);
                Settle();

                lock (gate)
                {
                    CompileHostProtocol.WriteAnswer(output, id, answer);
                }
            })
            {
                IsBackground = true,
                Name = "fivestar compile host",
                Priority = priority,
            };
            thread.Start();
        }
    }

    /// <summary>The backend's compile of <paramref name="request"/>; a heavy source waits while three others compile.</summary>
    private static BackendResult Compile(InProcessBackend backend, CompileRequest request)
    {
        if (request.Hlsl.Length < ProgramForm.HeavySourceLength)
        {
            return backend.Run(request);
        }

        Heavy.Wait();

        try
        {
            return backend.Run(request);
        }
        finally
        {
            Heavy.Release();
        }
    }

    /// <summary>
    /// After a request: when it was the last one running and more than <see cref="IdleHeapBytes"/> is committed, or more than <see cref="BusyHeapBytes"/> at most every 10 s, a
    /// compacting collection gives the memory back. Server GC kept 12 GB after a burst of 16 makes of mesh programs,
    /// which held program work to one at a time (the loader caps the host at 8 GB) and counted against the boot's memory.
    /// </summary>
    private static void Settle()
    {
        bool idle = Interlocked.Decrement(ref _active) == 0;

        // The committed bytes, not the heap size after the last collection: that stayed small in v2 while 10 GB stayed
        // committed for two minutes.
        long committed = GC.GetGCMemoryInfo().TotalCommittedBytes;
        long now = Environment.TickCount64;
        bool due = committed >= BusyHeapBytes && now - Interlocked.Read(ref _lastCollect) >= CollectIntervalMilliseconds;

        if (!(idle && committed >= IdleHeapBytes) && !due)
        {
            return;
        }

        Interlocked.Exchange(ref _lastCollect, now);
        System.Runtime.GCSettings.LargeObjectHeapCompactionMode = System.Runtime.GCLargeObjectHeapCompactionMode.CompactOnce;
        GC.Collect(GC.MaxGeneration, GCCollectionMode.Aggressive, blocking: true, compacting: true);
    }

    /// <summary>The answer to one request; an error (a file that cannot be written, no validator) is answered as such, so the loader takes the work.</summary>
    private static HostAnswer Serve(InProcessBackend backend, OnceLog warnings, CompileRequest? compile, MakeRequest? make)
    {
        try
        {
            if (compile is not null)
            {
                var result = Compile(backend, compile);

                return HostAnswer.Of(result.Verdict, result.Reason);
            }

            return Make(backend, warnings, make!);
        }
        catch (Exception exception) when (exception is not OutOfMemoryException)
        {
            return HostAnswer.Failed($"error: {exception.GetType().Name} {exception.Message}");
        }
    }

    /// <summary>Translates, picks the form and compiles it into <c>&lt;folder&gt;/&lt;name&gt;.dxil</c>; a form the backend cannot take comes back for dxc.</summary>
    private static HostAnswer Make(InProcessBackend backend, OnceLog warnings, MakeRequest make)
    {
        // One producer per folder and adapter: its program loader (the decoder's tables) is made once.
        var producer = Producers.GetOrAdd((make.Programs, make.Folder, make.Adapter, make.Header), _ =>
        {
            var picker = new ProgramVariantPicker(new ShaderManifest([]), make.Adapter, warnings);
            var compiler = new DxilBackendCompiler(DxcOptions.Defaults(string.Empty, retry: false), null, backend);

            var store = new FiveStar.Shaders.Cache.ProgramContentStore(FiveStar.Shaders.Cache.ProgramContentStore.FolderBeside(make.Folder), FiveStar.Shaders.Cache.ProgramContentStore.CompilerIdentity(compiler));

            return new ProgramProducer(make.Programs, make.Folder, picker, compiler, SlowCompiles.Read([], compiler.Options.CompileTimeout)) { Header = make.Header, ContentStore = store };
        });
        var form = producer.Prepare(make.Name);

        if (form.Failure is not null)
        {
            return HostAnswer.Refused(form.Failure);
        }

        var request = form.Request!;
        var store = producer.ContentStore;
        BackendResult result;

        // A program the translator still writes the same way is copied from the content store, not compiled again.
        if (store is not null && store.TryRestore(request))
        {
            result = new BackendResult(BackendVerdict.Compiled, string.Empty);
        }
        else
        {
            result = Compile(backend, request);

            if (result.Verdict == BackendVerdict.Compiled)
            {
                store?.Keep(request);
            }
        }

        if (result.Verdict == BackendVerdict.Compiled)
        {
            var target = FiveStar.Shaders.Cache.PlainFileName.Inside(make.Folder, make.Name + DxilExtension);

            if (!string.Equals(request.DxilPath, target, StringComparison.OrdinalIgnoreCase))
            {
                File.Move(request.DxilPath, target, overwrite: true);
            }

            return HostAnswer.Compiled;
        }

        return new HostAnswer((HostAnswerKind)result.Verdict, result.Reason, request.Profile, Path.GetFileName(request.HlslPath), request.Hlsl);
    }
}
