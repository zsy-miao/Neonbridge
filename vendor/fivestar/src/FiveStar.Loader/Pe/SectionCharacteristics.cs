// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Pe;

/// <summary>The section flags the loader reads (PE format specification, "Section Flags").</summary>
[Flags]
internal enum SectionCharacteristics : uint
{
    None = 0,
    ContainsCode = 0x00000020,
    ContainsInitializedData = 0x00000040,
    MemoryDiscardable = 0x02000000,
    MemoryExecute = 0x20000000,
    MemoryRead = 0x40000000,
    MemoryWrite = 0x80000000,
}
