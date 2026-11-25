// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;

namespace FiveStar.Loader.Native;

/// <summary>MEMORY_BASIC_INFORMATION (winnt.h), x64 layout: 0x30 bytes.</summary>
[StructLayout(LayoutKind.Sequential)]
internal struct MemoryBasicInformation
{
    public nint BaseAddress;
    public nint AllocationBase;
    public uint AllocationProtect;
    public ushort PartitionId;
    public ushort Padding1;
    public nuint RegionSize;
    public uint State;
    public uint Protect;
    public uint Type;
    public uint Padding2;
}
