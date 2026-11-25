// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims;

/// <summary>What the console GPU may do with memory, from the flags the game allocated it with (<see cref="GraphicsAllocations"/>).</summary>
internal enum GraphicsAccess
{
    /// <summary>No PAGE_GRAPHICS_* access and no MEM_GRAPHICS, or memory the game did not allocate through xmem.dll.</summary>
    None,

    /// <summary>PAGE_GRAPHICS_READONLY, _EXECUTE or _EXECUTE_READ.</summary>
    Read,

    /// <summary>PAGE_GRAPHICS_READWRITE or _EXECUTE_READWRITE, or MEM_GRAPHICS without a read-only graphics protection.</summary>
    ReadWrite,
}
