// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Pe;

/// <summary>IMAGE_REL_BASED_* types the loader applies (PE format specification, "Base Relocation Types").</summary>
internal enum BaseRelocationType
{
    Absolute = 0,
    High = 1,
    Low = 2,
    HighLow = 3,
    Dir64 = 10,
}
