// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Translation.Waves;

/// <summary>How a lane mask relates to the current EXEC mask (<c>active</c>), with U a wave-uniform bool.</summary>
internal enum MaskKind
{
    /// <summary>Nothing known.</summary>
    Other,

    /// <summary>The same on every lane: U.</summary>
    Uniform,

    /// <summary>active and U: on a wave that holds no active lane it is empty, whatever U is.</summary>
    ActiveAnd,

    /// <summary>not active, or not U: the negation of an <see cref="ActiveAnd"/> mask.</summary>
    NotActiveAnd,

    /// <summary>A subset of active, per lane.</summary>
    SubActive,
}
