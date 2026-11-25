// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;

namespace FiveStar.Loader.Calls;

/// <summary>Calls into Bink2Scarlett.dll, each checked against its disassembly first.</summary>
internal static unsafe class BinkRecipes
{
    public const string Module = "bink2scarlett.dll";

    /// <summary>BinkUtilCPUs is <c>mov eax,7; ret</c> in the Xbox build.</summary>
    private const int HardcodedCpuCount = 7;

    private const int TimerSleepMs = 100;
    private const int TimerToleranceMs = 10;
    private const int AllocationSize = 1000;
    private const byte FillPattern = 0xAB;
    private const long ReportedAlignmentCap = 64;

    public static ExportCallResult CpuCount(ExportCallContext call)
    {
        int count = ((delegate* unmanaged<int>)call.Function)();

        return new ExportCallResult($"returned {count} (Xbox build hardcodes {HardcodedCpuCount} CPUs)", count == HardcodedCpuCount);
    }

    /// <summary>BinkGetError returns its static error buffer; empty before any error.</summary>
    public static ExportCallResult GetError(ExportCallContext call)
    {
        nint text = ((delegate* unmanaged<nint>)call.Function)();
        string? value = Marshal.PtrToStringAnsi(text);

        return new ExportCallResult($"returned 0x{text:x} ({call.Runtime.Images.Describe(text)}) = \"{value}\"", value is not null && value.Length == 0);
    }

    /// <summary>RADTimerRead: once-initialized QueryPerformanceFrequency, then the QPC delta in milliseconds.</summary>
    public static ExportCallResult TimerRead(ExportCallContext call)
    {
        var read = (delegate* unmanaged<ulong>)call.Function;
        ulong before = read();
        Thread.Sleep(TimerSleepMs);
        ulong after = read();
        ulong delta = after - before;

        return new ExportCallResult($"t0={before} ms, t1={after} ms, delta={delta} ms over Thread.Sleep({TimerSleepMs})", delta >= TimerSleepMs - TimerToleranceMs);
    }

    /// <summary>
    /// BinkUtilMalloc(size) calls XMemAlloc(size + 0x40, 0x05E50000) and BinkUtilFree calls XMemFree. The allocation
    /// logs a stack walk from inside the shim, through the Bink frames, back to the caller.
    /// </summary>
    public static ExportCallResult MallocFree(ExportCallContext call)
    {
        var free = call.Image.Exports.Find("BinkUtilFree", 0);

        if (free is null)
        {
            return new ExportCallResult("BinkUtilFree missing", false);
        }

        var xmem = call.Runtime.XMem;
        long allocations = xmem.AllocationCount;
        long frees = xmem.FreeCount;
        xmem.TraceNextAllocation();
        nint block = ((delegate* unmanaged<nuint, nint>)call.Function)(AllocationSize);

        if (block == 0)
        {
            return new ExportCallResult("BinkUtilMalloc returned NULL", false);
        }

        new Span<byte>((void*)block, AllocationSize).Fill(FillPattern);
        ((delegate* unmanaged<nint, void>)free.Address)(block);
        long alignment = Math.Min(ReportedAlignmentCap, (long)block & -(long)block);
        long allocated = xmem.AllocationCount - allocations;
        long freed = xmem.FreeCount - frees;
        string summary = $"BinkUtilMalloc({AllocationSize}) = 0x{block:x} (aligned to >= {alignment}), XMemAlloc calls +{allocated}, BinkUtilFree -> XMemFree calls +{freed}";

        return new ExportCallResult(summary, allocated == 1 && freed == 1);
    }
}
