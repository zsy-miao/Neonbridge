// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;

namespace FiveStar.Loader.Shims.D3D12X;

/// <summary>D3D12_RESOURCE_DESC (d3d12.h); device slot 133 takes the same 56 bytes.</summary>
[StructLayout(LayoutKind.Sequential)]
internal struct ResourceDescription
{
    /// <summary>D3D12_RESOURCE_DIMENSION_BUFFER.</summary>
    public const int BufferDimension = 1;

    /// <summary>The D3D12_RESOURCE_FLAGS bits the desktop defines, ALLOW_RENDER_TARGET to RAYTRACING_ACCELERATION_STRUCTURE.</summary>
    private const int DesktopFlags = 0x1FF;

    /// <summary>D3D12_TEXTURE_LAYOUT_64KB_STANDARD_SWIZZLE, the highest desktop layout; the game also passes 0x100 + an XG swizzle mode.</summary>
    private const int MaxDesktopLayout = 3;

    /// <summary>D3D12_TEXTURE_LAYOUT_UNKNOWN: the driver's own layout.</summary>
    private const int UnknownLayout = 0;

    public int Dimension;
    public ulong Alignment;
    public ulong Width;
    public uint Height;
    public ushort DepthOrArraySize;
    public ushort MipLevels;
    public int Format;
    public uint SampleCount;
    public uint SampleQuality;
    public int Layout;
    public int Flags;

    /// <summary>
    /// The desc the desktop accepts: X-only flag bits (0x20000 on GPU-memory buffers, 0x300000 on textures) dropped
    /// and a console layout (0x100 + swizzle mode, set at 0x1403440f7) left to the driver.
    /// </summary>
    public readonly ResourceDescription ToDesktop()
    {
        var desktop = this;
        desktop.Flags &= DesktopFlags;

        if ((uint)desktop.Layout > MaxDesktopLayout)
        {
            desktop.Layout = UnknownLayout;
        }

        return desktop;
    }
}
