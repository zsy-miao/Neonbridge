// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.Storage;

/// <summary>
/// The compression byte of a request's options. The game builds it at 0x144ffd71a from a <c>.ttpf</c> page's flag byte:
/// bit 0 is the page's zlib bit; when that is clear, bits 1-3 are the page's BCPack codec (<c>DSTORAGE_BCPACK_MODE</c>,
/// 1 BC1, 3 BC3, 4 BC4, 5 BC5, 6 BC6H, 7 BC7), so a BC7 page reads 14. Archive chunks use 0 and 1.
/// </summary>
internal static class StorageCompression
{
    /// <summary>The bytes are copied as stored.</summary>
    public const byte None = 0;

    /// <summary>A zlib stream (RFC 1950).</summary>
    public const byte Zlib = 1;

    private const int BcPackModeShift = 1;
    private const int BcPackModeMask = 0x7;

    /// <summary>The BCPack codec of <paramref name="compression"/>, or 0 when it is not a BCPack format.</summary>
    [HotPath]
    public static int BcPackCodec(byte compression)
    {
        if ((compression & Zlib) != 0 || (compression >> BcPackModeShift) > BcPackModeMask)
        {
            return 0;
        }

        return compression >> BcPackModeShift;
    }
}
