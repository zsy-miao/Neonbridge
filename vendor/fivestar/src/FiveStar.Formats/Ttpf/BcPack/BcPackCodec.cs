// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Formats.Ttpf.BcPack;

/// <summary>The BCPack codecs: the page codec number, which equals <c>DSTORAGE_BCPACK_MODE</c> (BC2 does not occur).</summary>
public enum BcPackCodec
{
    /// <summary>BC1: 12 lanes, one symbol per lane per 8-byte block.</summary>
    Bc1 = 1,

    /// <summary>BC3: 12 lanes, two symbols per lane per 16-byte block.</summary>
    Bc3 = 3,

    /// <summary>BC4: 12 lanes, one symbol per lane per 8-byte block.</summary>
    Bc4 = 4,

    /// <summary>BC5: 12 lanes, two symbols per lane per 16-byte block.</summary>
    Bc5 = 5,

    /// <summary>BC6H: 16 lanes, two symbols per lane per 16-byte block.</summary>
    Bc6H = 6,

    /// <summary>BC7: 16 lanes, two symbols per lane per 16-byte block.</summary>
    Bc7 = 7,
}
