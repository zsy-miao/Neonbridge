// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Pe;

/// <summary>Base relocation block layout (PE format specification, "The .reloc Section").</summary>
internal static class RelocationFormat
{
    /// <summary>Block header: PageRVA and BlockSize, 32 bits each.</summary>
    public const int BlockHeaderSize = 8;
    public const int BlockSizeOffset = 4;
    public const int EntrySize = 2;
    public const int BlockAlignment = 4;

    /// <summary>The high 4 bits of an entry are the type, the low 12 bits the offset in the page.</summary>
    public const int TypeShift = 12;
    public const int OffsetMask = 0xFFF;
    public const uint PageMask = ~(uint)OffsetMask;

    /// <summary>Bits 16..31 of a value, for IMAGE_REL_BASED_HIGH.</summary>
    public const int HighWordShift = 16;
    public const long WordMask = 0xFFFF;
}
