// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Translation.Waves;

/// <summary>The HLSL type of a known <see cref="MissingValue"/>, which decides how operators treat its bits.</summary>
internal enum MissingType
{
    UInt,
    Int,
    Float,
    Bool,
}
