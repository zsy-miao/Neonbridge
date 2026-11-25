// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Dxil.Ir;

/// <summary>The attribute sets of a <c>dx.op</c> declaration; the value is the attribute list number in the module.</summary>
public enum AttributeSet
{
    None = 0,
    ReadNone = 1,
    NoUnwind = 2,
    ReadOnly = 3,

    /// <summary>Barriers: nounwind and noduplicate.</summary>
    NoDuplicate = 4,

    /// <summary>A pixel shader entry whose wave operations count helper lanes ("waveops-include-helper-lanes").</summary>
    HelperLanes = 5,
}
