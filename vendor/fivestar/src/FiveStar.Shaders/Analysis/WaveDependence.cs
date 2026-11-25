// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Analysis;

/// <summary>How much a translated program depends on the hardware wave size.</summary>
public enum WaveDependence
{
    /// <summary>No wave or quad intrinsic after the entry point.</summary>
    Agnostic,

    /// <summary>Only lane-index and EXEC bookkeeping (ballots, lane bits): fixable mechanically.</summary>
    Bookkeeping,

    /// <summary>Only quad operations and first-lane reads, which behave the same on any wave size.</summary>
    QuadAndFirstLane,

    /// <summary>Lane-indexed or wave-wide data movement that assumes AMD lane numbering.</summary>
    LaneIndexed,
}
