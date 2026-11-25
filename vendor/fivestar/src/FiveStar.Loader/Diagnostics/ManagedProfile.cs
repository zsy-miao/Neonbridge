// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Diagnostics;
using System.Globalization;
using FiveStar.Loader.Loading;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Diagnostics;

/// <summary>
/// <c>--profile-managed &lt;start&gt;[:&lt;seconds&gt;]</c>: from <c>start</c> seconds into the run, the runtime's CPU
/// sampler records every managed thread's stack once a millisecond for <c>seconds</c> (an EventPipe session of the
/// process itself, <see cref="EventPipeSession"/>), with the rundown that names the methods. Beside the .nettrace a
/// .threads.tsv gives each thread seen in the window its class (game, build, queue, loader, runtime, other) and name;
/// <c>fivestar profile summary</c> reads both. Sampling suspends the managed threads briefly each millisecond, so the
/// window is kept short and the default is off.
/// </summary>
internal sealed unsafe class ManagedProfile
{
    /// <summary>The thread table's extension, beside the trace.</summary>
    public const string ThreadsExtension = ".threads.tsv";

    private const double DefaultSeconds = 20;
    private const int StartAndLength = 2;
    private const int KibShift = 10;
    private const int NoteMilliseconds = 1000;
    private const uint QueryInformation = 0x40;
    private const int Win32StartAddressClass = 9;
    private const string Prefix = "  [profile] ";

    /// <summary>Thread names (by prefix or part) and their class; first match wins.</summary>
    private static readonly (string Part, string Class)[] NamedClasses =
    [
        ("mapped main", "game"),
        ("program compile", "build"),
        ("pipeline build", "build"),
        ("warm-up", "build"),
        ("compile host", "build"),
        ("pipeline cache", "build"),
        ("queue", "queue"),
        ("fence", "queue"),
        ("memory waits", "queue"),
        ("DirectStorage", "queue"),
        (".NET", "runtime"),
        ("fivestar", "loader"),
    ];

    private readonly LoadedImage? _main;
    private readonly string _path;
    private readonly TimeSpan _start;
    private readonly TimeSpan _length;
    private readonly LoaderLog _log;
    private readonly Dictionary<uint, (string Class, string Name)> _threads = [];

    private ManagedProfile(LoadedImage? main, string path, TimeSpan start, TimeSpan length, LoaderLog log)
    {
        _main = main;
        _path = path;
        _start = start;
        _length = length;
        _log = log;
    }

    /// <summary>Parses <c>&lt;start&gt;[:&lt;seconds&gt;]</c>; false when it is not two non-negative numbers.</summary>
    public static bool TryParse(string text, out TimeSpan start, out TimeSpan length)
    {
        var parts = text.Split(':');
        double seconds = DefaultSeconds;
        start = length = TimeSpan.Zero;

        if (parts.Length > StartAndLength || !double.TryParse(parts[0], NumberStyles.Float, CultureInfo.InvariantCulture, out double from) || from < 0)
        {
            return false;
        }

        if (parts.Length == StartAndLength && (!double.TryParse(parts[1], NumberStyles.Float, CultureInfo.InvariantCulture, out seconds) || seconds <= 0))
        {
            return false;
        }

        (start, length) = (TimeSpan.FromSeconds(from), TimeSpan.FromSeconds(seconds));

        return true;
    }

    /// <summary>Where a run's trace goes: beside its log, else in the working folder.</summary>
    public static string TracePath(string? logFile)
    {
        if (logFile is not null)
        {
            return Path.ChangeExtension(logFile, ".managed.nettrace");
        }

        return Path.Combine(Environment.CurrentDirectory, $"fivestar-managed-{Environment.ProcessId}.nettrace");
    }

    /// <summary>Starts the profile's own thread, which waits for the window.</summary>
    public static void Start(ImageTable images, string path, TimeSpan start, TimeSpan length, LoaderLog log)
    {
        var main = images.All.FirstOrDefault(image => !image.Pe.IsDll);
        var profile = new ManagedProfile(main, path, start, length, log);
        new Thread(profile.Run) { IsBackground = true, Name = "fivestar managed profile" }.Start();
    }

    /// <summary>A thread's class from its start address (the game's image) or its name.</summary>
    public static string Classify(bool startsInGame, string name)
    {
        if (startsInGame)
        {
            return "game";
        }

        foreach (var (part, kind) in NamedClasses)
        {
            if (name.Contains(part, StringComparison.OrdinalIgnoreCase))
            {
                return kind;
            }
        }

        return "other";
    }

    private void Run()
    {
        Thread.Sleep(_start);
        EventPipeSession session;

        try
        {
            session = EventPipeSession.Start(_path, [(EventPipeSession.SampleProfiler, 0, EventPipeSession.Verbose), (EventPipeSession.Runtime, EventPipeSession.MethodKeywords, EventPipeSession.Verbose)]);
        }
        catch (Exception exception) when (exception is IOException or TimeoutException or UnauthorizedAccessException)
        {
            _log.Warn($"{Prefix}no managed profile: {exception.Message}");

            return;
        }

        _log.Info($"{Prefix}managed samples for {_length.TotalSeconds:F0} s into {_path}");
        var clock = Stopwatch.StartNew();

        while (clock.Elapsed < _length)
        {
            NoteThreads();
            Thread.Sleep(NoteMilliseconds);
        }

        session.Stop();
        long bytes = session.Bytes;
        session.Dispose();

        string table = Path.ChangeExtension(_path, null) + ThreadsExtension;
        File.WriteAllLines(table, _threads.OrderBy(pair => pair.Key).Select(pair => $"{pair.Key}\t{pair.Value.Class}\t{pair.Value.Name}"));
        _log.Info($"{Prefix}wrote {_path} ({bytes >> KibShift} KiB) and {table}: fivestar profile summary {_path}");
    }

    /// <summary>The process's threads not seen yet, with their class and name.</summary>
    private void NoteThreads()
    {
        foreach (ProcessThread thread in Process.GetCurrentProcess().Threads)
        {
            uint id = (uint)thread.Id;

            if (!_threads.ContainsKey(id))
            {
                string name = ThreadStacks.Name(id);
                _threads[id] = (Classify(StartsInGame(id), name), name);
            }
        }
    }

    private bool StartsInGame(uint id)
    {
        nint thread = Kernel32.OpenThread(QueryInformation, 0, id);

        if (thread == 0 || _main is null)
        {
            if (thread != 0)
            {
                _ = Kernel32.CloseHandle(thread);
            }

            return false;
        }

        nint start = 0;
        bool known = Ntdll.NtQueryInformationThread(thread, Win32StartAddressClass, &start, sizeof(nint), null) == 0;
        _ = Kernel32.CloseHandle(thread);

        return known && _main.Contains(start);
    }
}
