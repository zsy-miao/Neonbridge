// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Pe;

/// <summary>One optional-header data directory: an RVA and a size.</summary>
internal readonly record struct DataDirectory(uint Rva, uint Size)
{
    public bool IsPresent => Rva != 0 && Size != 0;
}
