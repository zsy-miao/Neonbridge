// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;

namespace FiveStar.Loader.Native;

/// <summary>The kernelbase functions the loader calls.</summary>
internal static unsafe class KernelBase
{
    private const string Library = "kernelbase";

    /// <summary>BOOL IsApiSetImplemented(PCSTR contract) (apiquery2.h, Windows 10+): whether this OS answers the API set.</summary>
    [DllImport(Library)]
    public static extern int IsApiSetImplemented(byte* contract);

    /// <summary>VirtualAlloc2 (memoryapi.h, Windows 10 1803+); here only to reserve and replace placeholders.</summary>
    [DllImport(Library, SetLastError = true)]
    public static extern nint VirtualAlloc2(nint process, nint address, nuint size, uint allocationType, uint protect, void* parameters, uint parameterCount);

    [DllImport(Library, SetLastError = true)]
    public static extern nint CreateFileMapping2(nint file, void* security, uint access, uint protect, uint attributes, ulong maximumSize, char* name, void* parameters, uint parameterCount);

    [DllImport(Library, SetLastError = true)]
    public static extern nint MapViewOfFile3(nint section, nint process, nint address, ulong offset, nuint size, uint allocationType, uint protect, void* parameters, uint parameterCount);

    [DllImport(Library, SetLastError = true)]
    public static extern int UnmapViewOfFile2(nint process, nint address, uint flags);

    [DllImport(Library, SetLastError = true)]
    public static extern int GlobalMemoryStatusEx(MemoryStatusEx* status);

    /// <summary>
    /// QueryVirtualMemoryInformation (memoryapi.h, Windows 10 1803+); here with MemoryRegionInfo (0), which reads the
    /// allocation's own counters instead of walking its page tables as VirtualQuery does.
    /// </summary>
    [DllImport(Library)]
    public static extern int QueryVirtualMemoryInformation(nint process, nint address, int informationClass, MemoryRegionInformation* information, nuint size, nuint* returned);
}
