// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Formats.Ttpf;

/// <summary>Where and how a texture's mip 0 is stored: one image per cube face or array slice.</summary>
/// <param name="Format">The texture's format.</param>
/// <param name="Kind">The resource kind.</param>
/// <param name="BlockWidth">Width in elements.</param>
/// <param name="BlockHeight">Height in elements.</param>
/// <param name="Depth">Depth of a volume (1 otherwise).</param>
/// <param name="Pattern">The swizzle of mip 0 (a prefix of the tile pattern when mip 0 sits in the mip tail).</param>
/// <param name="Size">Bytes of one image of mip 0 padded to whole tiles of <paramref name="Pattern"/>.</param>
/// <param name="Start">Virtual address of the first image.</param>
/// <param name="Images">1, 6 for a cube map, or the array size.</param>
/// <param name="ImageStride">Bytes from one image to the next.</param>
public sealed record TextureLayout(
    TextureFormat Format,
    TextureKind Kind,
    int BlockWidth,
    int BlockHeight,
    int Depth,
    SwizzlePattern Pattern,
    long Size,
    long Start,
    int Images,
    long ImageStride);
