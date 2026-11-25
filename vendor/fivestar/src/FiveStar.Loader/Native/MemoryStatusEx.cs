// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;

namespace FiveStar.Loader.Native;

/// <summary>MEMORYSTATUSEX (sysinfoapi.h).</summary>
[StructLayout(LayoutKind.Sequential)]
internal struct MemoryStatusEx
{
    public uint Length;
    public uint MemoryLoad;
    public ulong TotalPhysical;
    public ulong AvailablePhysical;
    public ulong TotalPageFile;
    public ulong AvailablePageFile;
    public ulong TotalVirtual;
    public ulong AvailableVirtual;
    public ulong AvailableExtendedVirtual;
}
