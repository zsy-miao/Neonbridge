// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.D3D12X.Present;

/// <summary>
/// What one present shows: the plane's desktop resource, the state it rests in, the part of it to show, the back-buffer
/// format, and whether the plane can be copied or drawn from (otherwise the test pattern stands in). The resource's own
/// size and the format a shader view of it takes are for <see cref="PresentScaler"/>.
/// </summary>
internal readonly record struct PlaneSource(
    nint Resource,
    int State,
    CopyBox Box,
    int Format,
    uint ColorSpace,
    bool Copyable,
    int ViewFormat = SwapChainFormats.None,
    uint ResourceWidth = 0,
    uint ResourceHeight = 0)
{
    public uint Width => Box.Right - Box.Left;

    public uint Height => Box.Bottom - Box.Top;
}
