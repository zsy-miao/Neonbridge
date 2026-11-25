// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.D3D12X.Present;

/// <summary>
/// The flip-model back-buffer format a plane copies into (same typeless family, so CopyTextureRegion takes it), and
/// the color space that goes with it (dxgiformat.h, dxgicommon.h).
/// </summary>
internal static class SwapChainFormats
{
    /// <summary>DXGI_FORMAT_UNKNOWN: no flip-model format shares the plane's family.</summary>
    public const int None = 0;

    public const int Rgba16Float = 10;
    public const int Rgb10A2Unorm = 24;
    public const int Rgba8Unorm = 28;
    public const int Bgra8Unorm = 87;

    /// <summary>DXGI_COLOR_SPACE_RGB_FULL_G22_NONE_P709: SDR sRGB.</summary>
    public const uint Srgb = 0;

    /// <summary>DXGI_COLOR_SPACE_RGB_FULL_G10_NONE_P709: scRGB, linear FP16.</summary>
    public const uint ScRgb = 1;

    /// <summary>DXGI_COLOR_SPACE_RGB_FULL_G2084_NONE_P2020: HDR10.</summary>
    public const uint Hdr10 = 12;

    private const int Rgba16Typeless = 9;
    private const int Rgb10A2Typeless = 23;
    private const int Rgba8Typeless = 27;
    private const int Rgba8Srgb = 29;
    private const int Bgra8Typeless = 90;
    private const int Bgra8Srgb = 91;

    /// <summary>The back-buffer format for a plane format, or <see cref="None"/>.</summary>
    [HotPath]
    public static int ForPlane(int format)
    {
        switch (format)
        {
            case Rgba16Typeless:
            case Rgba16Float:
                return Rgba16Float;
            case Rgb10A2Typeless:
            case Rgb10A2Unorm:
                return Rgb10A2Unorm;
            case Rgba8Typeless:
            case Rgba8Unorm:
            case Rgba8Srgb:
                return Rgba8Unorm;
            case Bgra8Typeless:
            case Bgra8Unorm:
            case Bgra8Srgb:
                return Bgra8Unorm;
            default:
                return None;
        }
    }

    /// <summary>The format a shader view of a plane in <paramref name="format"/> takes: its own, the back-buffer one when typeless.</summary>
    [HotPath]
    public static int ViewFormat(int format)
    {
        switch (format)
        {
            case Rgba16Typeless:
            case Rgb10A2Typeless:
            case Rgba8Typeless:
            case Bgra8Typeless:
                return ForPlane(format);
            default:
                return format;
        }
    }

    /// <summary>
    /// The swap chain's color space: scRGB for FP16, HDR10 for 10-bit with the plane's dword equal to the HDR10
    /// value (*inferred*: the dword read as a DXGI_COLOR_SPACE_TYPE), else sRGB.
    /// </summary>
    [HotPath]
    public static uint ColorSpace(int backBufferFormat, uint planeColorSpace)
    {
        if (backBufferFormat == Rgba16Float)
        {
            return ScRgb;
        }

        if (backBufferFormat == Rgb10A2Unorm && planeColorSpace == Hdr10)
        {
            return Hdr10;
        }

        return Srgb;
    }
}
