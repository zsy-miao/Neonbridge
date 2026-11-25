// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;

namespace FiveStar.Loader.Shims;

/// <summary>One batch of CopyPageMappingsBatchX: the ranges (<see cref="PageRange"/>) mapped from a destination address on.</summary>
[StructLayout(LayoutKind.Sequential)]
internal unsafe struct PageBatch
{
    public ulong Destination;
    public uint Count;
    public PageRange* Ranges;
}
