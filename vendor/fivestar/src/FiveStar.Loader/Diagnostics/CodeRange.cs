// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Diagnostics;

/// <summary>The address range of one module, mapped or OS-loaded, whose unwind data a stack walk may use.</summary>
internal readonly record struct CodeRange(ulong Start, ulong End)
{
    public bool Contains(ulong address)
    {
        return address >= Start && address < End;
    }
}
