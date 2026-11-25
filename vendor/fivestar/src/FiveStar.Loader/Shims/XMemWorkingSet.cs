// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims;

/// <summary>
/// <c>BOOL XMemGetWorkingSetStatistics(DWORD workingSetFlags, XMEM_WORKING_SET_STATISTICS* statistics)</c> (GDK
/// reference). The structure's layout is not public; from the callers: they zero 0x78 bytes, and one subtracts the
/// second qword from the first and compares the result with 100 MiB, so the first pair is the title's total and used
/// bytes. Here total is the resident set plus the free physical memory and used is the resident set, so total - used
/// is the memory still free. The other fields stay zero.
/// </summary>
internal sealed unsafe class XMemWorkingSet
{
    /// <summary>The bytes callers zero before the call (four 32-byte stores ending at +0x78).</summary>
    private const int StatisticsSize = 0x78;

    private const int TotalField = 0;
    private const int UsedField = 1;

    private readonly LoaderLog _log;

    public XMemWorkingSet(LoaderLog log)
    {
        _log = log;
    }

    public int GetStatistics(uint flags, byte* statistics)
    {
        if (statistics == null)
        {
            return 0;
        }

        ProcessMemoryCountersEx counters = default;
        counters.Size = (uint)sizeof(ProcessMemoryCountersEx);
        MemoryStatusEx status = default;
        status.Length = (uint)sizeof(MemoryStatusEx);

        if (Kernel32.K32GetProcessMemoryInfo(Kernel32.GetCurrentProcess(), &counters, counters.Size) == 0 || KernelBase.GlobalMemoryStatusEx(&status) == 0)
        {
            return 0;
        }

        new Span<byte>(statistics, StatisticsSize).Clear();
        var fields = (ulong*)statistics;
        fields[TotalField] = counters.WorkingSetSize + status.AvailablePhysical;
        fields[UsedField] = counters.WorkingSetSize;
        _log.Detail($"  [xmem] XMemGetWorkingSetStatistics(0x{flags:x}) -> total=0x{fields[TotalField]:x} used=0x{fields[UsedField]:x}");

        return 1;
    }
}
