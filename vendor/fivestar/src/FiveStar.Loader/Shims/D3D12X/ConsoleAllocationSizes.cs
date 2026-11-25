// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Shims.Xg;

namespace FiveStar.Loader.Shims.D3D12X;

/// <summary>
/// Sizes of textures in a console layout (0x100 + swizzle mode) as the console reports them: the XG layout's size and
/// alignment (docs/xg.md), not the desktop driver's. The game sizes streamed textures with slot 25 (0x140343e80) and
/// reads exactly that many bytes from its archives, whose compressed chunks were cut to the console size; a smaller
/// desktop size ends a read inside a chunk and the game breaks at 0x144ffd879.
/// </summary>
internal static unsafe class ConsoleAllocationSizes
{
    /// <summary>D3D12_TEXTURE_LAYOUT_UNKNOWN.</summary>
    private const int UnknownLayout = 0;

    /// <summary>ALLOW_RENDER_TARGET, ALLOW_DEPTH_STENCIL and ALLOW_UNORDERED_ACCESS.</summary>
    private const int TargetFlags = 0x7;

    /// <summary>DXGI_SAMPLE_DESC.Count of a texture that is not multisampled.</summary>
    private const uint SingleSample = 1;

    /// <summary>
    /// Replaces <paramref name="result"/> (and <paramref name="infos"/>, when given) with the console sizes when every
    /// desc is a texture the console sizes with XG (<see cref="IsConsoleSized"/>); false, leaving them as they are,
    /// otherwise.
    /// </summary>
    public static bool TryApply(ResourceDescription* descriptions, uint count, ResourceAllocationInfo* result, ResourceAllocationInfo1* infos)
    {
        if (count == 0 || !AllConsoleSized(descriptions, count))
        {
            return false;
        }

        ulong offset = 0;
        ulong alignment = 1;

        for (uint i = 0; i < count; i++)
        {
            XgLayoutBuilder.Build(*(XgResourceDesc*)(descriptions + i), out var surface);
            offset = XgLayoutBuilder.AlignUp(offset, surface.Alignment);

            if (infos != null)
            {
                infos[i] = new ResourceAllocationInfo1 { Offset = offset, Alignment = surface.Alignment, Size = surface.Size };
            }

            offset += surface.Size;
            alignment = Math.Max(alignment, surface.Alignment);
        }

        result->Size = XgLayoutBuilder.AlignUp(offset, alignment);
        result->Alignment = alignment;

        return true;
    }

    /// <summary>
    /// The console size of a texture the game creates committed, render and depth targets included (XG lays those out
    /// with the mode it suggests); false for buffers and multisampled textures, which keep the desktop size.
    /// </summary>
    public static bool TryCommittedTextureSize(ResourceDescription* description, out ulong size)
    {
        size = 0;

        if (description->Dimension == ResourceDescription.BufferDimension || description->SampleCount > SingleSample)
        {
            return false;
        }

        XgLayoutBuilder.Build(*(XgResourceDesc*)description, out var surface);
        size = surface.Size;

        return true;
    }

    /// <summary>
    /// Whether a desc is a texture the console lays out with XG: its layout field carries a swizzle mode, or it is
    /// UNKNOWN on a texture that is neither render target, depth target nor UAV (XG suggests the mode; the game sizes
    /// streamed textures so, 0x1451fbd80 builds their desc without a layout).
    /// </summary>
    public static bool IsConsoleTexture(in ResourceDescription description)
    {
        if (description.Dimension == ResourceDescription.BufferDimension)
        {
            return false;
        }

        return (uint)description.Layout >= XgSwizzleModes.ModeBase || (description.Layout == UnknownLayout && (description.Flags & TargetFlags) == 0);
    }

    /// <summary>
    /// Whether the console reports XG's size for a desc: a texture in a console layout, or a single-sample render
    /// target, depth target or UAV texture without a layout. The game places those in its 2.2 GiB graphics heap at the
    /// size slot 108 answers (0x140abfad0), while the shim backs them with desktop textures of their own, so the
    /// desktop driver's size only takes heap: AMD's (RX 7800 XT) adds its compression metadata, 0x1330000 bytes for a
    /// 2560x1440 depth target where XG gives 0xb40000, and the story load ran the heap out at a 46 MiB request.
    /// </summary>
    public static bool IsConsoleSized(in ResourceDescription description)
    {
        if (IsConsoleTexture(description))
        {
            return true;
        }

        return description.Dimension != ResourceDescription.BufferDimension && description.Layout == UnknownLayout && description.SampleCount <= SingleSample;
    }

    private static bool AllConsoleSized(ResourceDescription* descriptions, uint count)
    {
        for (uint i = 0; i < count; i++)
        {
            if (!IsConsoleSized(descriptions[i]))
            {
                return false;
            }
        }

        return true;
    }
}
