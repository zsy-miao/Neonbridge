// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;

namespace FiveStar.Loader.Native;

/// <summary>HEAP_SUMMARY (heapapi.h): Size is set by the caller.</summary>
[StructLayout(LayoutKind.Sequential)]
internal struct HeapSummaryInfo
{
    public uint Size;
    public nuint Allocated;
    public nuint Committed;
    public nuint Reserved;
    public nuint MaxReserve;
}
