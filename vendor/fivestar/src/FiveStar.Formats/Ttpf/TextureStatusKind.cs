// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Formats.Ttpf;

/// <summary>Why a texture's mip 0 can or cannot be read.</summary>
public enum TextureStatusKind
{
    /// <summary>Layout understood and every page under mip 0 can be decoded.</summary>
    Ok,

    /// <summary>The DXGI format is not supported (detail = format).</summary>
    UnsupportedFormat,

    /// <summary>The layout mode has no known swizzle (detail = mode).</summary>
    UnsupportedMode,

    /// <summary>The resource kind is not known (detail = kind).</summary>
    UnsupportedKind,

    /// <summary>The computed layout runs past the texture's span.</summary>
    Size,

    /// <summary>A cube map with more copies than its mip chain has units.</summary>
    CubeCopies,

    /// <summary>Mip 0 touches a page that cannot be decoded (a BCPack codec other than BC1).</summary>
    UnsupportedCodec,

    /// <summary>Coverage only: mip 0 touches a BCPack page that cannot be decoded (detail = codec).</summary>
    BcPack,

    /// <summary>Coverage only: mip 0 reads, but through a BCPack codec whose decode is approximate (detail = codec).</summary>
    Approximate,
}
