// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Formats.Ttpf.BcPack;

/// <summary>Outcome of a BCPack decode.</summary>
public enum BcPackDecodeStatus
{
    /// <summary>All lanes converged: exact symbol counts, legal end states, only zero padding left.</summary>
    Ok,

    /// <summary>Bad arguments (block count, output size).</summary>
    InvalidArgument,

    /// <summary>A fragment header selects an alphabet index above 8, which the hardware does not define.</summary>
    BadHeader,

    /// <summary>No lane made progress for 64 clocks: the stream ran out or is not of this codec.</summary>
    NoProgress,

    /// <summary>The clock budget ran out.</summary>
    CycleLimit,

    /// <summary>Every lane finished, but some ended outside the legal states L, L / 2^8 and L / 2^16.</summary>
    IllegalEndState,

    /// <summary>The stream ended before the chunk did and the rest is not zero padding.</summary>
    TrailingData,

    /// <summary>The lanes converged, but some block records name no valid block (an unused BC7 control code or BC6H mode).</summary>
    InvalidBlock,
}
