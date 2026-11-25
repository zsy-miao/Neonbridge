// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Loading;
using FiveStar.Loader.Native;
using FiveStar.Loader.Pe;

namespace FiveStar.Loader.Host;

/// <summary>
/// Runs a mapped executable's entry point (the CRT startup, which runs the static initializers and then main) on a
/// thread whose stack is the size the image asks for. The CRT normally ends the process through exit(). With a time
/// limit, a run that has not ended by then has its main thread suspended and reported, and the loader exits.
/// </summary>
internal sealed unsafe class ExecutableRunner
{
    public const int TimedOut = 3;

    private const string ThreadName = "mapped main";

    // winnt.h: THREAD_SUSPEND_RESUME | THREAD_GET_CONTEXT | THREAD_QUERY_INFORMATION.
    private const uint ThreadAccess = 0x0002 | 0x0008 | 0x0040;

    // CONTEXT (winnt.h, AMD64): size, alignment, ContextFlags offset, CONTEXT_FULL.
    private const int ContextSize = 1232;
    private const int ContextAlignment = 16;
    private const int ContextFlagsOffset = 0x30;
    private const uint ContextFull = 0x10000B;

    private readonly FaultReporter _faults;
    private readonly LoaderLog _log;
    private uint _threadId;

    public ExecutableRunner(FaultReporter faults, LoaderLog log)
    {
        _faults = faults;
        _log = log;
    }

    /// <param name="image">The executable.</param>
    /// <param name="limit">How long the entry point may run; null waits for it.</param>
    public int Run(LoadedImage image, TimeSpan? limit)
    {
        int stack = checked((int)StackReserve(image));
        int result = 0;
        var thread = new Thread(() => result = Enter(image), stack) { Name = ThreadName, IsBackground = true };
        _log.Info($"== running {image.Name} entry RVA 0x{image.Pe.EntryPointRva:x} on a thread with a 0x{stack:x}-byte stack");
        thread.Start();

        if (limit is null)
        {
            thread.Join();
        }
        else if (!thread.Join(limit.Value))
        {
            ReportHung(limit.Value);

            return TimedOut;
        }

        _log.Info($"entry point returned {result}");

        return result;
    }

    /// <summary>SizeOfStackReserve from the mapped optional header.</summary>
    private static ulong StackReserve(LoadedImage image)
    {
        byte* headers = (byte*)image.Base;
        int ntHeader = *(int*)(headers + PeFormat.NewHeaderPointerOffset);
        byte* optional = headers + ntHeader + PeFormat.NtSignatureSize + PeFormat.FileHeaderSize;

        return *(ulong*)(optional + PeFormat.SizeOfStackReserveOffset);
    }

    private int Enter(LoadedImage image)
    {
        Volatile.Write(ref _threadId, Kernel32.GetCurrentThreadId());

        return EntryPoints.RunExecutable(image);
    }

    private void ReportHung(TimeSpan limit)
    {
        uint id = Volatile.Read(ref _threadId);

        // The calls first: writing the record of a suspended thread can block on a lock that thread holds.
        _faults.WriteRecentCalls($"TIMEOUT after {limit.TotalSeconds} s");
        nint thread = Kernel32.OpenThread(ThreadAccess, 0, id);
        byte* raw = stackalloc byte[ContextSize + ContextAlignment];
        byte* context = (byte*)(((ulong)raw + ContextAlignment - 1) & ~(ulong)(ContextAlignment - 1));
        *(uint*)(context + ContextFlagsOffset) = ContextFull;

        if (thread == 0 || Kernel32.SuspendThread(thread) == uint.MaxValue || Kernel32.GetThreadContext(thread, context) == 0)
        {
            _log.Error($"main thread still running after {limit.TotalSeconds} s; its state could not be read");

            return;
        }

        _faults.WriteThreadState($"TIMEOUT: main thread still running after {limit.TotalSeconds} s", id, context);
    }
}
