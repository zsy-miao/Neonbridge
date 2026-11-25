// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Text;
using FiveStar.Debugger.Native;

namespace FiveStar.Debugger.Session;

/// <summary>Writes exceptions: one line each, or a full record with registers and symbolised stacks.</summary>
internal sealed unsafe class ExceptionReporter
{
    private const int RegistersPerLine = 8;

    /// <summary>ExceptionInformation[0] of an access violation: the access that faulted (winnt.h).</summary>
    private const ulong ReadAccess = 0;
    private const ulong WriteAccess = 1;
    private const ulong ExecuteAccess = 8;

    /// <summary>An access violation carries the access and the address.</summary>
    private const int AccessParameters = 2;

    private static readonly string[] RegisterNames =
        ["rax", "rcx", "rdx", "rbx", "rsp", "rbp", "rsi", "rdi", "r8", "r9", "r10", "r11", "r12", "r13", "r14", "r15"];

    private readonly DebugLog _log;
    private readonly StackWalker _walker;

    public ExceptionReporter(DebugLog log, StackWalker walker)
    {
        _log = log;
        _walker = walker;
    }

    /// <summary><c>1234/5678 first-chance c0000005 AccessViolation at 0x... ntdll.dll+0x... ntdll!memset+0x2a (write at 0x10)</c>.</summary>
    public static string Summary(DebuggeeProcess process, uint threadId, ExceptionRecord record, bool firstChance)
    {
        string chance = "second-chance";

        if (firstChance)
        {
            chance = "first-chance";
        }

        return $"{process.Id}/{threadId} {chance} {ExceptionCodes.Describe(record.Code)} at 0x{record.Address:x} {process.Describe(record.Address)}{Access(record)}";
    }

    public void Line(DebuggeeProcess process, uint threadId, ExceptionRecord record, bool firstChance)
    {
        _log.Write(Summary(process, threadId, record, firstChance));
    }

    /// <summary>The summary, the registers and stack of the thread, and with <paramref name="allThreads"/> every other thread's stack.</summary>
    public void Record(DebuggeeProcess process, uint threadId, ExceptionRecord record, bool firstChance, bool allThreads)
    {
        _log.Write("== " + Summary(process, threadId, record, firstChance));

        if (process.Threads.TryGetValue(threadId, out nint thread))
        {
            ThreadReport(process, threadId, thread, true);
        }

        if (!allThreads)
        {
            return;
        }

        foreach (var (otherId, other) in process.Threads)
        {
            if (otherId != threadId)
            {
                ThreadReport(process, otherId, other, false);
            }
        }
    }

    private static string Access(ExceptionRecord record)
    {
        bool memoryFault = record.Code == (uint)WellKnownException.AccessViolation || record.Code == (uint)WellKnownException.InPageError;

        if (!memoryFault || record.NumberParameters < AccessParameters)
        {
            return string.Empty;
        }

        string kind = record.Information[0] switch
        {
            ReadAccess => "read",
            WriteAccess => "write",
            ExecuteAccess => "execute",
            _ => $"access {record.Information[0]}",
        };

        return $" ({kind} at 0x{record.Information[1]:x})";
    }

    private void ThreadReport(DebuggeeProcess process, uint threadId, nint thread, bool withRegisters)
    {
        using var context = ThreadContext.Capture(thread);

        if (context is null)
        {
            _log.Write($"   thread {threadId}: no context");

            return;
        }

        if (withRegisters)
        {
            Registers(context);
        }

        _log.Write($"   thread {threadId} stack:");
        var frames = _walker.Walk(process.Handle, thread, context);

        for (int i = 0; i < frames.Count; i++)
        {
            _log.Write($"     #{i:d2} 0x{frames[i]:x16} {process.Describe(frames[i])}");

            // Without an image there is no unwind data, so the frames dbghelp guesses past this one are noise.
            if (process.ModuleAt(frames[i]) is null)
            {
                _log.Write("     (no image here, JIT code or a stub: the walk stops)");

                break;
            }
        }
    }

    private void Registers(ThreadContext context)
    {
        var line = new StringBuilder();

        for (int i = 0; i < Amd64Context.GeneralRegisterCount; i++)
        {
            line.Append($"{RegisterNames[i]}={context.Pointer->GeneralRegisters[i]:x16} ");

            if ((i + 1) % RegistersPerLine == 0)
            {
                _log.Write("   " + line.ToString().TrimEnd());
                line.Clear();
            }
        }

        _log.Write($"   rip={context.Pointer->Rip:x16} efl={context.Pointer->EFlags:x8}");
    }
}
