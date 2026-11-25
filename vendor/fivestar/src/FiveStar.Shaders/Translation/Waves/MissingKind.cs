// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Translation.Waves;

/// <summary>How much <see cref="MissingLaneAnalysis"/> knows of a value.</summary>
internal enum MissingKind
{
    /// <summary>Any value.</summary>
    Unknown,

    /// <summary>The value in <see cref="MissingValue.Bits"/>.</summary>
    Known,

    /// <summary>A lane number found in a lane mask: a lane with a pixel.</summary>
    MaskLane,

    /// <summary>Some lane number computed from this lane's: below <see cref="MissingValue.Bits"/>.</summary>
    LaneNumber,
}
