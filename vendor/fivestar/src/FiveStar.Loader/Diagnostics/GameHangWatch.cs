// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Diagnostics;
using FiveStar.Loader.Loading;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Diagnostics;

/// <summary>
/// <c>--watch-hang</c>: follows the game's hang watchdog and the state it ends the game from. The watchdog thread
/// (0x142215a60) checks once a second that the main loop's heartbeat (a QueryPerformanceCounter value, written by
/// 0x1422159f0) has moved; after 30 checks without a move it calls the handler at 0x1471b6348, if any, and writes to
/// address 0. The watch logs the heartbeat's stalls, the main state machine's state, the loading-screen flags and the
/// handler as they change, and every thread's stack while the heartbeat stands still. It reads game memory only.
/// </summary>
internal sealed unsafe class GameHangWatch
{
    /// <summary>Game globals (1.0.2.1).</summary>
    private const ulong HeartbeatAddress = 0x1471B6338;
    private const ulong WatchdogPausedAddress = 0x1471B6330;
    private const ulong WatchdogHandlerAddress = 0x1471B6348;
    private const ulong MainStatesAddress = 0x1471273A0;
    private const ulong LoadingScreenAddress = 0x147453ED4;
    private const ulong SessionPointerAddress = 0x14AAE3B70;
    private const ulong LoadingRenderAddress = 0x14A210D60;
    private const ulong LoadingRenderOtherAddress = 0x14A210D70;

    /// <summary>The game's fence-wait loop (polls a fence's completed-value cell, fence slot 12).</summary>
    private const ulong FenceWaitStart = 0x144FB0AD0;
    private const ulong FenceWaitEnd = 0x144FB0E90;

    /// <summary>The state machine's current and requested state, 16-bit each (0x1453ebea0).</summary>
    private const int StateOffset = 0x10;
    private const int NextStateOffset = 0x12;
    private const int SessionReadyOffset = 0x1B3;

    private const ulong LowestUserAddress = 0x10000;
    private const int PollMilliseconds = 100;
    private const int MaxFrames = 64;
    private const string Prefix = "  [hang] ";

    /// <summary>Heartbeat ages at which every thread's stack is logged; the watchdog fires at about 30 s.</summary>
    private static readonly double[] DumpAfterSeconds = [5, 15, 25];

    /// <summary>A stall shorter than this is not logged when the heartbeat moves again.</summary>
    private const double StallSeconds = 2;

    private readonly ThreadStacks _stacks;
    private readonly ImageTable _images;
    private readonly Func<string> _counters;
    private readonly Func<ulong, ulong, string> _fenceWait;
    private readonly LoaderLog _log;
    private readonly Stopwatch _clock = Stopwatch.StartNew();

    private GameHangWatch(ImageTable images, Func<string> counters, Func<ulong, ulong, string> fenceWait, LoaderLog log)
    {
        _stacks = new ThreadStacks(images, MaxFrames);
        _images = images;
        _counters = counters;
        _fenceWait = fenceWait;
        _log = log;
    }

    /// <summary>
    /// Starts the watch thread. <paramref name="counters"/> gives the loader counters logged with stacks,
    /// <paramref name="fenceWait"/> the fence whose completed-value cell is at an address and the work a wait on it for a value depends on.
    /// </summary>
    public static void Start(ImageTable images, Func<string> counters, Func<ulong, ulong, string> fenceWait, LoaderLog log)
    {
        var watch = new GameHangWatch(images, counters, fenceWait, log);
        var thread = new Thread(watch.Run) { IsBackground = true, Name = "fivestar hang watch" };
        thread.Start();
        log.Info($"{Prefix}watching the game's heartbeat, main state and loading-screen flags");
    }

    private void Run()
    {
        string last = string.Empty;
        ulong beat = 0;
        double beatAt = 0;
        int dumps = 0;
        var waitsAtBeat = BlockingWaits.Snapshot();

        while (true)
        {
            Thread.Sleep(PollMilliseconds);
            double now = _clock.Elapsed.TotalSeconds;
            string state = Describe();

            if (state != last)
            {
                _log.Info($"{Prefix}{now:F1} s: {state}");
                last = state;
            }

            ulong current = *(ulong*)HeartbeatAddress;

            if (current != beat)
            {
                double stalled = now - beatAt;

                if (beat != 0 && stalled >= StallSeconds)
                {
                    _log.Info($"{Prefix}{now:F1} s: heartbeat moved again after {stalled:F1} s");
                }

                beat = current;
                beatAt = now;
                dumps = 0;
                waitsAtBeat = BlockingWaits.Snapshot();

                continue;
            }

            if (beat == 0 || dumps == DumpAfterSeconds.Length || now - beatAt < DumpAfterSeconds[dumps])
            {
                continue;
            }

            dumps++;
            string stall = BlockingWaits.Summary("loader waits since the heartbeat last moved, all threads", BlockingWaits.Snapshot(), waitsAtBeat);
            _log.Info($"{Prefix}{now:F1} s: heartbeat still for {now - beatAt:F1} s; {stall}; {BlockingWaits.Summary()}; {_counters()}");
            _stacks.Log(_log, Prefix);
            LogWaitObjects();
            LogFenceWaits();
        }
    }

    /// <summary>The game's events and semaphores that blocked threads wait on (<see cref="WaitObjects"/>), when any.</summary>
    private void LogWaitObjects()
    {
        string report = WaitObjects.Report(_images, Prefix);

        if (report.Length != 0)
        {
            _log.Info(report);
        }
    }

    /// <summary>
    /// Each thread in the game's fence-wait loop (0x144fb0ad0, which keeps the target value in RSI and the polled cell in
    /// RDI): the value it waits for, the cell's value now and the fence behind the cell (last Signal, desktop value).
    /// </summary>
    private void LogFenceWaits()
    {
        var modules = _stacks.Modules();
        var wait = new CodeRange(FenceWaitStart, FenceWaitEnd);

        foreach (ProcessThread thread in Process.GetCurrentProcess().Threads)
        {
            if (!ThreadStackCapture.FrameRegisters((uint)thread.Id, modules, wait, out ulong target, out ulong cell) || !IsReadable(cell))
            {
                continue;
            }

            _log.Info($"{Prefix}  thread {thread.Id} waits at 0x144fb0ad0 for {target} in cell 0x{cell:x} (now {*(ulong*)cell}); {_fenceWait(cell, target)}");
        }
    }

    /// <summary>
    /// Whether the game is loading: its loading-screen flag is set, or it started less than
    /// <paramref name="young"/> ago (<paramref name="age"/>); false without the game's image.
    /// </summary>
    public static bool Loading(TimeSpan age, TimeSpan young)
    {
        if (!IsReadable(LoadingScreenAddress))
        {
            return false;
        }

        return age < young || *(byte*)LoadingScreenAddress != 0;
    }

    private static string Describe()
    {
        ushort state = *(ushort*)(MainStatesAddress + StateOffset);
        ushort next = *(ushort*)(MainStatesAddress + NextStateOffset);
        byte loading = *(byte*)LoadingScreenAddress;
        byte paused = *(byte*)WatchdogPausedAddress;
        ulong handler = *(ulong*)WatchdogHandlerAddress;

        int loadingRender = 0;

        if (*(ulong*)LoadingRenderAddress != 0 || *(ulong*)LoadingRenderOtherAddress != 0)
        {
            loadingRender = 1;
        }

        ulong session = *(ulong*)SessionPointerAddress;
        string ready = "no session";

        if (IsReadable(session + SessionReadyOffset))
        {
            ready = (*(byte*)(session + SessionReadyOffset)).ToString(System.Globalization.CultureInfo.InvariantCulture);
        }

        return $"main state {(short)state} (next {(short)next}), loading screen {loading}, loading render callback {loadingRender}, session 0x{session:x} ready {ready}, watchdog paused {paused}, handler 0x{handler:x}";
    }

    /// <summary>Whether <paramref name="address"/> is committed and accessible in this process.</summary>
    internal static bool IsReadable(ulong address)
    {
        MemoryBasicInformation info;

        if (address < LowestUserAddress || Kernel32.VirtualQueryEx(Kernel32.GetCurrentProcess(), (nint)address, &info, (nuint)sizeof(MemoryBasicInformation)) == 0)
        {
            return false;
        }

        bool blocked = (info.Protect & (Win32Constants.PageNoAccess | Win32Constants.PageGuard)) != 0;

        return info.State == Win32Constants.MemCommit && !blocked;
    }
}
