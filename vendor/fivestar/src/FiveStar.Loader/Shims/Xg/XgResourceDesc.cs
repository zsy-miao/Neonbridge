// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;

namespace FiveStar.Loader.Shims.Xg;

/// <summary>
/// The description XGCreateTextureComputer and XGSuggestSwizzleMode read: the D3D12_RESOURCE_DESC layout (0x38 bytes).
/// The game builds it field by field at 0x144ecc940 (and a copy at 0x140c4b000); <see cref="Layout"/> holds
/// 0x100 + the swizzle mode, or what XGSuggestSwizzleMode wrote there.
/// </summary>
[StructLayout(LayoutKind.Sequential)]
internal struct XgResourceDesc
{
    /// <summary>D3D12_RESOURCE_DIMENSION: 2 = 1D, 3 = 2D, 4 = 3D.</summary>
    public uint Dimension;
    public ulong Alignment;
    public ulong Width;
    public uint Height;
    public ushort DepthOrArraySize;
    public ushort MipLevels;
    public uint Format;
    public uint SampleCount;
    public uint SampleQuality;
    public uint Layout;
    public uint Flags;
}
