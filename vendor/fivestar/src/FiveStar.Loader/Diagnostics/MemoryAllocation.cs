// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Diagnostics;

/// <summary>
/// One VirtualAlloc/MapViewOfFile allocation of the memory map: its committed bytes by type, the span it reserves,
/// the protection of its first committed region and the dword at +0x10 of its base when that is readable (heap
/// segments carry a signature there).
/// </summary>
internal sealed class MemoryAllocation
{
    public MemoryAllocation(nint allocationBase)
    {
        Base = allocationBase;
    }

    public nint Base { get; }

    public long Private { get; set; }

    public long Mapped { get; set; }

    public long Image { get; set; }

    public long Reserved { get; set; }

    public uint Protect { get; set; }

    public uint Signature { get; set; }

    public string Label { get; set; } = "";
}
