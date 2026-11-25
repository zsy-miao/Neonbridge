// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Formats.Pso;

/// <summary>Where a PSO's tables come from.</summary>
public enum PsoLayout
{
    /// <summary>An L-format PSO file: tables in chunks (<c>PSIN LSCH LENM LSNS LGID LRSC CHKS</c>).</summary>
    File,

    /// <summary>A PRD0 paged resource: the same tables reached through virtual pointers.</summary>
    Resource,
}
