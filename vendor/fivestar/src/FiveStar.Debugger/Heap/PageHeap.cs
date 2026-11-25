// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Debugger.Native;

namespace FiveStar.Debugger.Heap;

/// <summary>
/// Turns on ntdll's page heap for one process: at the create-process event no user-mode code has run yet, and ntdll
/// reads PEB.NtGlobalFlag when it creates the heaps, so setting FLG_HEAP_PAGE_ALLOCS there scopes the page heap to this
/// process without the image-file-execution-options registry key.
/// </summary>
internal static unsafe class PageHeap
{
    /// <summary>FLG_HEAP_PAGE_ALLOCS (gflags): every heap allocation in its own pages, followed by a guard page.</summary>
    private const uint HeapPageAllocations = 0x02000000;

    /// <summary>PROCESSINFOCLASS ProcessBasicInformation (winternl.h).</summary>
    private const int ProcessBasicInformationClass = 0;

    /// <summary>sizeof(PROCESS_BASIC_INFORMATION) on x64.</summary>
    private const int BasicInformationSize = 0x30;

    /// <summary>PROCESS_BASIC_INFORMATION.PebBaseAddress.</summary>
    private const int PebBaseAddressOffset = 0x8;

    /// <summary>PEB.NtGlobalFlag (x64, public ntdll symbols).</summary>
    private const int NtGlobalFlagOffset = 0xBC;

    /// <summary>Sets the flag; false when the PEB cannot be read or written.</summary>
    public static bool Enable(nint process)
    {
        byte* information = stackalloc byte[BasicInformationSize];
        int length;

        if (Ntdll.NtQueryInformationProcess(process, ProcessBasicInformationClass, information, BasicInformationSize, &length) != 0)
        {
            return false;
        }

        ulong flagAddress = *(ulong*)(information + PebBaseAddressOffset) + NtGlobalFlagOffset;

        if (!RemoteMemory.TryRead(process, flagAddress, out uint flags))
        {
            return false;
        }

        return RemoteMemory.TryWrite(process, flagAddress, flags | HeapPageAllocations);
    }
}
