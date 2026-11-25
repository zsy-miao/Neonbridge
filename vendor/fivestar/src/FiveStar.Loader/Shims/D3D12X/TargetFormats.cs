// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.D3D12X;

/// <summary>
/// Render-target substitutes (dxgiformat.h): the console renders to and stores 16-bit color formats that a desktop GPU
/// may not (B4G4R4A4_UNORM has no render-target support on NVIDIA, B5G5R5A1_UNORM no typed UAV). A render-target or UAV
/// texture in such a format is made R8G8B8A8_UNORM, which every desktop GPU renders to, loads and stores as a typed UAV; views, clear values and
/// pipeline target formats follow. The channels read back the same RGBA, with 8 bits where the console keeps 4 or 5
/// (blends and writes are not quantized as on the console). B5G6R5 has no alpha: a view of the substitute reads
/// alpha as 1 (<see cref="ComponentMapping"/>).
/// </summary>
internal static class TargetFormats
{
    public const int B5G6R5Unorm = 85;
    public const int B5G5R5A1Unorm = 86;
    public const int B4G4R4A4Unorm = 115;
    public const int R8G8B8A8Unorm = 28;

    /// <summary>D3D12_SHADER_COMPONENT_MAPPING: 3 bits per output component, alpha source 3, FORCE_VALUE_1 5.</summary>
    private const int ComponentBits = 3;
    private const int ComponentMask = 0x7;
    private const int Components = 4;
    private const int AlphaSource = 3;
    private const int ForceOne = 5;

    /// <summary>Whether <paramref name="format"/> has a substitute.</summary>
    public static bool HasSubstitute(int format)
    {
        return format is B5G6R5Unorm or B5G5R5A1Unorm or B4G4R4A4Unorm;
    }

    /// <summary>The substitute of <paramref name="format"/>; the format itself when it has none.</summary>
    public static int Substitute(int format)
    {
        if (HasSubstitute(format))
        {
            return R8G8B8A8Unorm;
        }

        return format;
    }

    /// <summary>
    /// The format the GPU writes for the console format <paramref name="format"/> on <paramref name="device"/>: the
    /// substitute when the device lacks render-target or typed UAV support for it. Render-target and UAV textures are
    /// made in it and pipelines render to it, so both always agree.
    /// </summary>
    public static int GpuWritten(nint device, int format)
    {
        if (!HasSubstitute(format))
        {
            return format;
        }

        uint support = FormatSupport.Support1(device, format);

        if ((support & FormatSupport.RenderTarget) == 0 || (support & FormatSupport.TypedUnorderedAccess) == 0)
        {
            return Substitute(format);
        }

        return format;
    }

    /// <summary>The format of a view in <paramref name="viewFormat"/> on a desktop resource in <paramref name="resourceFormat"/>.</summary>
    [HotPath]
    public static int View(int viewFormat, int resourceFormat)
    {
        if (HasSubstitute(viewFormat) && resourceFormat == Substitute(viewFormat))
        {
            return resourceFormat;
        }

        return viewFormat;
    }

    /// <summary>An SRV's component mapping for a view in <paramref name="viewFormat"/>: alpha reads 1 for B5G6R5.</summary>
    [HotPath]
    public static int ComponentMapping(int viewFormat, int mapping)
    {
        if (viewFormat != B5G6R5Unorm)
        {
            return mapping;
        }

        for (int component = 0; component < Components; component++)
        {
            int shift = component * ComponentBits;

            if (((mapping >> shift) & ComponentMask) == AlphaSource)
            {
                mapping = (mapping & ~(ComponentMask << shift)) | (ForceOne << shift);
            }
        }

        return mapping;
    }
}
