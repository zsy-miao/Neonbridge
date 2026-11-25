// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Diagnostics;
using System.Runtime;
using System.Runtime.InteropServices;
using FiveStar.Loader.Loading;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Diagnostics;

/// <summary>
/// <c>--profile-threads</c>: with each thread sample, a burst of stack captures of every thread; the threads that are
/// running (innermost frame not in a waiting system call) count towards the functions of the main image on their
/// stack. The busiest functions, inclusive and innermost, show what the game does each frame while it waits for
/// something. Each running sample is also placed by module: the innermost frame's module, and on game threads (a stack
/// that reaches the main image in some round) whether the game is in its own code or inside ours (the walk ended at JIT
/// code, which is only ever entered through the loader's shims). The busiest threads list how full they are.
/// </summary>
internal sealed unsafe class ThreadProfiler
{
    private const int Rounds = 400;
    private const int GapMilliseconds = 2;
    private const int MaxFrames = 48;
    private const int Top = 40;
    private const int TopThreads = 16;
    private const int TopModules = 6;
    private const long NoGcBudget = 16L << 20;
    /// <summary>A system-call stub: mov r10, rcx; mov eax, n; test; jne; syscall; ret; int 2e; ret.</summary>
    private const ulong StubBytes = 0x20;
    private const string Managed = "managed";
    private const double Percent = 100.0;
    private const string Prefix = "  [profile] ";

    private static readonly (string Module, string[] Calls)[] WaitCalls =
    [
        ("ntdll.dll", ["NtWaitForSingleObject", "NtWaitForMultipleObjects", "NtWaitForAlertByThreadId", "NtDelayExecution", "NtRemoveIoCompletion", "NtRemoveIoCompletionEx", "NtWaitForWorkViaWorkerFactory", "NtSignalAndWaitForSingleObject", "NtWaitForKeyedEvent", "NtReplyWaitReceivePort", "NtAlpcSendWaitReceivePort"]),
        ("win32u.dll", ["NtUserMsgWaitForMultipleObjectsEx", "NtUserGetMessage", "NtUserWaitMessage", "NtGdiDdDDIWaitForSynchronizationObjectFromCpu", "NtGdiDdDDIWaitForVerticalBlankEvent2"]),
    ];

    private readonly ImageTable _images;
    private readonly LoaderLog _log;
    private readonly Dictionary<string, string> _ours = new(StringComparer.Ordinal);

    public ThreadProfiler(ImageTable images, LoaderLog log)
    {
        _images = images;
        _log = log;
    }

    public void Run(CodeRange[] modules)
    {
        var main = _images.All.FirstOrDefault(i => !i.Pe.IsDll);

        if (main is null)
        {
            return;
        }

        var waits = WaitStubs();
        var names = ModuleNames();
        var inclusive = new Dictionary<uint, int>();
        var innermost = new Dictionary<uint, int>();
        int running = 0;

        uint self = Kernel32.GetCurrentThreadId();
        uint[] threads = [.. Process.GetCurrentProcess().Threads.Cast<ProcessThread>().Select(t => (uint)t.Id).Where(id => id != self)];
        var frames = new ulong[threads.Length * MaxFrames];
        var counts = new int[threads.Length];
        var tallies = new ThreadTally[threads.Length];
        var clock = Stopwatch.StartNew();

        for (int i = 0; i < tallies.Length; i++)
        {
            tallies[i] = new ThreadTally();
        }

        try
        {
            for (int round = 0; round < Rounds; round++)
            {
                CaptureRound(threads, modules, frames, counts);

                for (int i = 0; i < threads.Length; i++)
                {
                    var stack = frames.AsSpan(i * MaxFrames, counts[i]);
                    tallies[i].Game |= Reaches(main, stack);

                    if (counts[i] > 0 && !InRanges(stack[0], waits))
                    {
                        running++;
                        Count(main, stack, inclusive, innermost);
                        tallies[i].Add(Place(main, stack, modules, names));
                    }
                }

                Thread.Sleep(GapMilliseconds);
            }
        }
        finally
        {
            EndNoGcRegion();
        }

        _log.Info($"{Prefix}{running} running-thread samples over {Rounds} rounds of {threads.Length} threads in {clock.ElapsedMilliseconds} ms");
        _log.Info($"{Prefix}inclusive: {Format(inclusive)}");
        _log.Info($"{Prefix}innermost: {Format(innermost)}");
        LogPlaces(threads, tallies);
    }

    /// <summary>
    /// Totals by place over all threads and over game threads, then the busiest threads with how many rounds found
    /// them running and their places.
    /// </summary>
    private void LogPlaces(uint[] threads, ThreadTally[] tallies)
    {
        var all = new Dictionary<string, int>(StringComparer.Ordinal);
        var game = new Dictionary<string, int>(StringComparer.Ordinal);

        foreach (var tally in tallies)
        {
            foreach (var (place, count) in tally.Places)
            {
                all[place] = all.GetValueOrDefault(place) + count;

                if (tally.Game)
                {
                    game[place] = game.GetValueOrDefault(place) + count;
                }
            }
        }

        _log.Info($"{Prefix}all threads by place: {Shares(all, Top)}");
        _log.Info($"{Prefix}game threads by place: {Shares(game, Top)}");

        foreach (int i in Enumerable.Range(0, threads.Length).OrderByDescending(i => tallies[i].Running).Take(TopThreads))
        {
            var tally = tallies[i];

            if (tally.Running == 0)
            {
                break;
            }

            string name = ThreadStacks.Name(threads[i]);
            string kind = Kind(tally.Game, name);
            _log.Info($"{Prefix}thread {threads[i]} ({kind}) running {Percent * tally.Running / Rounds:F0} % of rounds: {Shares(tally.Places, TopModules)}");
        }
    }

    /// <summary>
    /// Where a running stack is: "game" in the main image; "ours: managed" in JIT or loader code; "ours: m" in module m
    /// called from the loader's code; otherwise the innermost module (called by the game or the OS).
    /// </summary>
    private string Place(LoadedImage main, ReadOnlySpan<ulong> stack, CodeRange[] modules, (CodeRange Range, string Name)[] names)
    {
        string inner = NameOf(stack[0], names);

        if (main.Contains((nint)stack[0]))
        {
            return "game";
        }

        if (inner == Managed)
        {
            return Ours(Managed);
        }

        foreach (ulong pc in stack)
        {
            if (!InRanges(pc, modules) || NameOf(pc, names) == Managed)
            {
                return Ours(inner);
            }

            if (main.Contains((nint)pc))
            {
                break;
            }
        }

        return inner;
    }

    /// <summary>"ours: m", made once per module: the burst allocates as little as it can inside its no-GC region.</summary>
    private string Ours(string module)
    {
        if (!_ours.TryGetValue(module, out var place))
        {
            place = $"ours: {module}";
            _ours[module] = place;
        }

        return place;
    }

    private static string Kind(bool game, string name)
    {
        if (game)
        {
            return "game";
        }

        if (name.Length == 0)
        {
            return "other";
        }

        return $"\"{name}\"";
    }

    private static bool Reaches(LoadedImage main, ReadOnlySpan<ulong> stack)
    {
        foreach (ulong pc in stack)
        {
            if (main.Contains((nint)pc))
            {
                return true;
            }
        }

        return false;
    }

    private static bool InRanges(ulong pc, CodeRange[] ranges)
    {
        foreach (var range in ranges)
        {
            if (range.Contains(pc))
            {
                return true;
            }
        }

        return false;
    }

    /// <summary>The module's file name; <see cref="Managed"/> for JIT code and the runtime's precompiled assemblies.</summary>
    private static string NameOf(ulong pc, (CodeRange Range, string Name)[] names)
    {
        foreach (var (range, name) in names)
        {
            if (range.Contains(pc))
            {
                return name;
            }
        }

        return Managed;
    }

    private (CodeRange Range, string Name)[] ModuleNames()
    {
        var names = _images.All.Select(i => (new CodeRange((ulong)i.Base, (ulong)i.Base + i.Size), i.Name)).ToList();

        foreach (ProcessModule module in Process.GetCurrentProcess().Modules)
        {
            string name = module.ModuleName;

            if (IsAssembly(name))
            {
                name = Managed;
            }

            names.Add((new CodeRange((ulong)module.BaseAddress, (ulong)module.BaseAddress + (ulong)module.ModuleMemorySize), name));
        }

        return [.. names];
    }

    /// <summary>Precompiled managed code: the loader's and the framework's assemblies.</summary>
    private static bool IsAssembly(string module)
    {
        return module.StartsWith("FiveStar.", StringComparison.OrdinalIgnoreCase)
            || module.StartsWith("System.", StringComparison.OrdinalIgnoreCase)
            || module.StartsWith("Microsoft.", StringComparison.OrdinalIgnoreCase)
            || module.StartsWith("fivestar-", StringComparison.OrdinalIgnoreCase);
    }

    private static string Shares(Dictionary<string, int> counts, int top)
    {
        int total = counts.Values.Sum();

        if (total == 0)
        {
            return "none";
        }

        return string.Join(", ", counts.OrderByDescending(p => p.Value).Take(top).Select(p => $"{p.Key} {Percent * p.Value / total:F1} %"));
    }

    /// <summary>
    /// One capture of every thread, inside a no-GC region (see <see cref="ThreadSampler"/>). The region stays open
    /// across rounds and is only started again when it lapsed: starting one runs a full blocking GC, and one per
    /// round stalled every managed shim, the present path included, for most of the burst.
    /// </summary>
    private static void CaptureRound(uint[] threads, CodeRange[] modules, ulong[] frames, int[] counts)
    {
        if (GCSettings.LatencyMode != GCLatencyMode.NoGCRegion)
        {
            // A region whose budget ran out has left the latency mode but is still open until it is ended.
            EndNoGcRegion();
            _ = GC.TryStartNoGCRegion(NoGcBudget);
        }

        for (int i = 0; i < threads.Length; i++)
        {
            counts[i] = ThreadStackCapture.Capture(threads[i], modules, frames.AsSpan(i * MaxFrames, MaxFrames));
        }
    }

    private static void EndNoGcRegion()
    {
        try
        {
            GC.EndNoGCRegion();
        }
        catch (InvalidOperationException)
        {
            // No region open, or it ended by a collection: either way none is open now.
        }
    }

    private static void Count(LoadedImage main, ReadOnlySpan<ulong> frames, Dictionary<uint, int> inclusive, Dictionary<uint, int> innermost)
    {
        var seen = new HashSet<uint>();
        bool first = true;

        foreach (ulong pc in frames)
        {
            if (!main.Contains((nint)pc))
            {
                continue;
            }

            uint start = FunctionStart(pc, main);

            if (first)
            {
                innermost[start] = innermost.GetValueOrDefault(start) + 1;
                first = false;
            }

            if (seen.Add(start))
            {
                inclusive[start] = inclusive.GetValueOrDefault(start) + 1;
            }
        }
    }

    /// <summary>The RVA of the .pdata function around <paramref name="pc"/>, else the RVA itself.</summary>
    private static uint FunctionStart(ulong pc, LoadedImage main)
    {
        ulong imageBase = 0;
        nint function = Ntdll.RtlLookupFunctionEntry(pc, &imageBase, 0);

        if (function == 0)
        {
            return (uint)(pc - (ulong)main.Base);
        }

        return *(uint*)function;
    }

    /// <summary>
    /// The system-call stubs a blocked thread sits in. Any other innermost frame runs, in user mode or in the kernel
    /// (VirtualQuery's walk, for one, is inside ntdll's ZwQueryVirtualMemory).
    /// </summary>
    private static CodeRange[] WaitStubs()
    {
        var stubs = new List<CodeRange>();

        foreach (var (module, calls) in WaitCalls)
        {
            if (!NativeLibrary.TryLoad(module, out nint handle))
            {
                continue;
            }

            foreach (string call in calls)
            {
                if (NativeLibrary.TryGetExport(handle, call, out nint stub))
                {
                    stubs.Add(new CodeRange((ulong)stub, (ulong)stub + StubBytes));
                }
            }
        }

        return [.. stubs];
    }

    private static string Format(Dictionary<uint, int> counts)
    {
        return string.Join(", ", counts.OrderByDescending(p => p.Value).Take(Top).Select(p => $"0x{p.Key:x} x{p.Value}"));
    }

    /// <summary>One thread's running samples by place, and whether any of its stacks reached the main image.</summary>
    private sealed class ThreadTally
    {
        public Dictionary<string, int> Places { get; } = new(StringComparer.Ordinal);

        public int Running { get; private set; }

        public bool Game { get; set; }

        public void Add(string place)
        {
            Running++;
            Places[place] = Places.GetValueOrDefault(place) + 1;
        }
    }
}
