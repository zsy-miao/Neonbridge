// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Native;

namespace FiveStar.Loader.Diagnostics;

/// <summary>
/// Walks the process's address space with VirtualQueryEx: every allocation with its committed bytes by type, and the
/// committed private bytes by protection.
/// </summary>
internal sealed unsafe class MemoryRegionWalk
{
    // MEMORY_BASIC_INFORMATION.Type (winnt.h).
    private const uint PrivateType = 0x20000;
    private const uint MappedType = 0x40000;
    private const uint ImageType = 0x1000000;

    /// <summary>MEMORY_BASIC_INFORMATION.State of unallocated space (MEM_FREE).</summary>
    private const uint FreeState = 0x10000;

    /// <summary>Where heap segments keep their signature (HEAP_SEGMENT and SEGMENT_HEAP, +0x10).</summary>
    private const int SignatureOffset = 0x10;

    /// <summary>Protections the CPU can read: READONLY, READWRITE, WRITECOPY, EXECUTE_READ, _READWRITE, _WRITECOPY.</summary>
    private const uint ReadableProtections = 0xEE;

    /// <summary>The top of the x64 user address space.</summary>
    private const long UserSpaceEnd = 0x7FFF_FFFF_0000;

    public Dictionary<nint, MemoryAllocation> Allocations { get; } = [];

    public Dictionary<uint, long> PrivateByProtection { get; } = [];

    public long Regions { get; private set; }

    public static MemoryRegionWalk Run()
    {
        var walk = new MemoryRegionWalk();
        nint process = Kernel32.GetCurrentProcess();
        nint address = 0;
        MemoryBasicInformation info;

        while (address < UserSpaceEnd && Kernel32.VirtualQueryEx(process, address, &info, (nuint)sizeof(MemoryBasicInformation)) != 0)
        {
            walk.Add(in info);
            address = info.BaseAddress + (nint)info.RegionSize;
        }

        return walk;
    }

    private void Add(in MemoryBasicInformation info)
    {
        if (info.State == FreeState || info.AllocationBase == 0)
        {
            return;
        }

        Regions++;

        if (!Allocations.TryGetValue(info.AllocationBase, out var allocation))
        {
            allocation = new MemoryAllocation(info.AllocationBase);
            Allocations[info.AllocationBase] = allocation;
        }

        long size = (long)info.RegionSize;
        allocation.Reserved += size;

        if (info.State != Win32Constants.MemCommit)
        {
            return;
        }

        if (allocation.Protect == 0)
        {
            allocation.Protect = info.Protect;
            allocation.Signature = ReadSignature(in info);
        }

        AddCommitted(allocation, info.Type, info.Protect, size);
    }

    private void AddCommitted(MemoryAllocation allocation, uint type, uint protect, long size)
    {
        if (type == PrivateType)
        {
            allocation.Private += size;
            PrivateByProtection[protect] = PrivateByProtection.GetValueOrDefault(protect) + size;
        }
        else if (type == MappedType)
        {
            allocation.Mapped += size;
        }
        else if (type == ImageType)
        {
            allocation.Image += size;
        }
    }

    /// <summary>The dword at +0x10 of the allocation base, when the base region is committed and readable.</summary>
    private static uint ReadSignature(in MemoryBasicInformation info)
    {
        if (info.BaseAddress != info.AllocationBase || (info.Protect & ReadableProtections) == 0 || (info.Protect & Win32Constants.PageGuard) != 0)
        {
            return 0;
        }

        uint signature = 0;
        nuint read = 0;

        // ReadProcessMemory rather than a load: another thread may free the allocation since the query.
        _ = Kernel32.ReadProcessMemory(Kernel32.GetCurrentProcess(), info.BaseAddress + SignatureOffset, &signature, sizeof(uint), &read);

        return signature;
    }
}
