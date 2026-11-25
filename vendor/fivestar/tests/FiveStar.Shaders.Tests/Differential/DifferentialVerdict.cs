// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Tests.Differential;

/// <summary>How the two control-flow forms of one program compared.</summary>
public enum DifferentialVerdict
{
    /// <summary>Both forms ran and every output word is equal.</summary>
    Agree,

    /// <summary>The outputs differ, or only one form compiled or ran.</summary>
    Mismatch,

    /// <summary>
    /// The outputs differ only through dxc's default fast-math optimizations (reassociation, contraction), which treat the
    /// two code shapes differently: compiled with IEEE strictness (-Gis) the forms agree.
    /// </summary>
    FastMath,

    /// <summary>A form gave different outputs in two runs (a race), so the comparison means nothing.</summary>
    Nondeterministic,

    /// <summary>The structured form is the same text (no branches, or the graph keeps the dispatch loop).</summary>
    SameText,

    /// <summary>Not compared: not translatable, not wrappable, or both forms failed the same way.</summary>
    Skipped,
}
