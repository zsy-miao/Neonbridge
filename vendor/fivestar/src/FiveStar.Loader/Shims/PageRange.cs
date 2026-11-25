// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;

namespace FiveStar.Loader.Shims;

/// <summary>One run of destination pages of a CopyPageMappingsBatchX batch (<see cref="PageBatch"/>): how they map and how many.</summary>
[StructLayout(LayoutKind.Sequential)]
internal struct PageRange
{
    /// <summary>Kinds: the pool pages from <see cref="Start"/> in order, pool page <see cref="Start"/> at every page, unmapped, unchanged.</summary>
    public const uint PoolPages = 0;
    public const uint OnePoolPage = 1;
    public const uint Unmapped = 2;
    public const uint Unchanged = 3;

    public uint Kind;
    public uint Start;
    public uint Count;
}
