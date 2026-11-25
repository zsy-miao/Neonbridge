// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Shims.Xg;

namespace FiveStar.Loader.Shims.D3D12X.Resources;

/// <summary>
/// A desktop texture standing for console memory at <see cref="Address"/>: <see cref="Layout"/> bytes in the console
/// tiling (docs/xg.md). The registry holds one reference on <see cref="Desktop"/>. A GPU-written texture (render
/// target, depth-stencil or UAV) is never filled from memory; any other is refreshed when the bytes changed.
/// </summary>
internal sealed class GpuTexture
{
    public GpuTexture(ulong address, ResourceDescription console, nint desktop, int desktopFormat, int state, bool gpuWritten, in XgSurface layout)
    {
        Address = address;
        Console = console;
        Desktop = desktop;

        DesktopFormat = desktopFormat;
        State = state;
        GpuWritten = gpuWritten;
        Layout = layout;
    }

    /// <summary>The console GPU address of the first byte.</summary>
    public ulong Address { get; }

    /// <summary>The resource desc the game gave (console layout and flags included).</summary>
    public ResourceDescription Console { get; }

    /// <summary>The desktop ID3D12Resource (one reference owned by the registry).</summary>
    public nint Desktop { get; }

    /// <summary>The DXGI format the desktop texture was created with.</summary>
    public int DesktopFormat { get; }

    /// <summary>The D3D12_RESOURCE_STATES the texture rests in between uploads.</summary>
    public int State { get; }

    public bool GpuWritten { get; }

    /// <summary>The console layout of the texture's bytes.</summary>
    public XgSurface Layout { get; }

    /// <summary>Bytes the texture spans in console memory.</summary>
    public ulong Size => Layout.Size;

    /// <summary>Whether the CPU maps the texture's memory, so it is checked for new bytes every frame (<see cref="GpuTextures.Watch"/>).</summary>
    public bool Watched { get; set; }

    /// <summary>
    /// Whether a GPU copy (list slots 85, 86) wrote the desktop texture. Its contents are then on the desktop, not in
    /// the console memory (on the console the copy wrote that memory), so a refresh from memory would undo the copy.
    /// </summary>
    public bool CopiedInto { get; set; }

    /// <summary>The content hash of the bytes last uploaded; 0 before the first upload.</summary>
    public ulong UploadedHash { get; set; }

    public bool Contains(ulong address)
    {
        return address - Address < Size;
    }

    /// <summary>Whether the game's desc describes this texture (same address, extent, format and mips; the layout field differs between slots).</summary>
    public bool Matches(ulong address, in ResourceDescription description)
    {
        return SameExtent(address, description) && description.Format == Console.Format;
    }

    /// <summary>Whether the game's desc has this texture's address, dimension, size and mips, whatever its format.</summary>
    public bool SameExtent(ulong address, in ResourceDescription description)
    {
        return address == Address && description.Dimension == Console.Dimension && description.Width == Console.Width
            && description.Height == Console.Height && description.DepthOrArraySize == Console.DepthOrArraySize
            && description.MipLevels == Console.MipLevels;
    }
}
