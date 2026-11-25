// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.D3D12X.Resources;

/// <summary>What a shader view loses when the desktop view takes another format than the game asked for (<see cref="ViewFormats.Loss"/>).</summary>
internal enum ViewFormatLoss
{
    /// <summary>The view has the game's format.</summary>
    None,

    /// <summary>Another member of the texture's cast family: the sRGB decode, or the integer or signed reading.</summary>
    Cast,

    /// <summary>A format of another family: the texture's bytes read as another format.</summary>
    Reinterpret,
}
