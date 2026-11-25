// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Native;

/// <summary>Placeholder, section and console page values (winnt.h, memoryapi.h, handleapi.h), named as in the SDK.</summary>
internal static class MemoryConstants
{
    // winnt.h: VirtualAlloc2 / MapViewOfFile3 allocation types.
    public const uint MemReplacePlaceholder = 0x00004000;
    public const uint MemReservePlaceholder = 0x00040000;
    public const uint MemPhysical = 0x00400000;
    public const uint MemLargePages = 0x20000000;

    /// <summary>winnt.h MEM_64K_PAGES: MEM_LARGE_PAGES | MEM_PHYSICAL.</summary>
    public const uint Mem64KPages = MemLargePages | MemPhysical;

    // winnt.h: VirtualFree free types that work on placeholders.
    public const uint MemCoalescePlaceholders = 0x00000001;
    public const uint MemPreservePlaceholder = 0x00000002;

    // winnt.h: UnmapViewOfFile2 flags.
    public const uint MemUnmapPreservePlaceholder = MemPreservePlaceholder;

    /// <summary>winnt.h PAGE_GRAPHICS_NOACCESS (0x800) through PAGE_GRAPHICS_NOCACHE (0x40000): the GPU view of a page.</summary>
    public const uint PageGraphicsMask = 0x0007F800;

    /// <summary>winnt.h SEC_COMMIT: a pagefile-backed section committed when created.</summary>
    public const uint SecCommit = 0x08000000;

    // memoryapi.h: section access rights.
    public const uint FileMapWrite = 0x0002;
    public const uint FileMapRead = 0x0004;

    /// <summary>handleapi.h INVALID_HANDLE_VALUE: as a file handle, CreateFileMapping2 backs the section by the pagefile.</summary>
    public const nint InvalidHandleValue = -1;
}
