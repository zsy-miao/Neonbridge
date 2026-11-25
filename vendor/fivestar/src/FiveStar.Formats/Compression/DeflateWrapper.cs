// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Formats.Compression;

/// <summary>The framing around a deflate stream.</summary>
public enum DeflateWrapper
{
    /// <summary>RFC 1950: two header bytes, the deflate data, a big-endian Adler-32 of the output.</summary>
    Zlib,

    /// <summary>RFC 1951 deflate data with no header or trailer.</summary>
    Raw,
}
