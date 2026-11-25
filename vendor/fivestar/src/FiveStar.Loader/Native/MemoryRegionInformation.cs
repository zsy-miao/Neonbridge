// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;

namespace FiveStar.Loader.Native;

/// <summary>
/// WIN32_MEMORY_REGION_INFORMATION (memoryapi.h), x64 layout: a whole allocation (one VAD), its size and how much of it
/// is committed.
/// </summary>
[StructLayout(LayoutKind.Sequential)]
internal struct MemoryRegionInformation
{
    /// <summary>The Private bit of <see cref="Flags"/>: VirtualAlloc memory, not a mapped view or an image.</summary>
    public const uint PrivateFlag = 0x1;

    public nint AllocationBase;
    public uint AllocationProtect;
    public uint Flags;
    public nuint RegionSize;
    public nuint CommitSize;
    public nuint PartitionId;
    public nuint NodePreference;

    /// <summary>
    /// The allocation around <paramref name="address"/> in this process; false when there is none. About a microsecond
    /// at any size, where VirtualQuery walks the page tables to size its region: 4 ms in a 2.2 GiB committed heap
    /// with half its pages touched.
    /// </summary>
    public static unsafe bool Query(nint address, out MemoryRegionInformation region)
    {
        MemoryRegionInformation info;
        nuint returned = 0;
        bool found = KernelBase.QueryVirtualMemoryInformation(Kernel32.GetCurrentProcess(), address, 0, &info, (nuint)sizeof(MemoryRegionInformation), &returned) != 0;
        region = found ? info : default;

        return found;
    }

    /// <summary>Whether this is private memory committed in full from its base, so every byte of it is committed.</summary>
    public readonly bool FullyCommittedPrivate => (Flags & PrivateFlag) != 0 && CommitSize == RegionSize && RegionSize != 0;

    /// <summary>Whether [<paramref name="start"/>, + <paramref name="size"/>) lies inside the allocation.</summary>
    public readonly bool Holds(nint start, ulong size)
    {
        ulong offset = (ulong)(start - AllocationBase);

        return start >= AllocationBase && offset <= RegionSize && size <= RegionSize - offset;
    }
}
