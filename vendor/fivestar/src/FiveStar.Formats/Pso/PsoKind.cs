// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Formats.Pso;

/// <summary>What a carved blob is, judged from its first 0x20 bytes.</summary>
public enum PsoKind
{
    /// <summary>Not a PSO and not a paged resource.</summary>
    None,

    /// <summary>A PSO file (<c>PSIN</c>).</summary>
    Psin,

    /// <summary>A PSO-backed paged resource (<c>PRD0</c> at +0x10).</summary>
    Prd0,

    /// <summary>Another paged resource's virtual segment.</summary>
    Rsc,
}
