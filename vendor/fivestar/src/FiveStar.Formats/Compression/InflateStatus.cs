// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Formats.Compression;

/// <summary>How an inflate call ended.</summary>
public enum InflateStatus
{
    /// <summary>The stream ended with its final block (and, for zlib, a matching Adler-32 trailer).</summary>
    Complete,

    /// <summary>The input ran out before the end of the stream; the output holds every byte decoded so far.</summary>
    Truncated,

    /// <summary>The stream would produce more than the allowed output.</summary>
    OutputLimit,

    /// <summary>The data is not a valid stream (bad header, code table, distance or checksum).</summary>
    Invalid,
}
