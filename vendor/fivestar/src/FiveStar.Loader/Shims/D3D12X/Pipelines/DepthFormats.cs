// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.D3D12X.Pipelines;

/// <summary>
/// Desktop formats for the console's depth formats (dxgiformat.h). Three are console-only: 118, D16_UNORM_S8_UINT,
/// which the desktop lacks and the game treats like D32_FLOAT_S8X24_UINT (XgFormats), and its plane views 119
/// (R16_UNORM_X8_TYPELESS, depth) and 120 (X16_TYPELESS_G8_UINT, stencil). They become the D32_FLOAT_S8X24_UINT
/// family: resources R32G8X24_TYPELESS (so depth-stencil views and SRVs of either plane both work), depth-stencil
/// views, pipeline DSVFormats and clear values D32_FLOAT_S8X24_UINT, depth-plane SRVs R32_FLOAT_X8X24_TYPELESS and
/// stencil-plane SRVs X32_TYPELESS_G8X24_UINT.
/// </summary>
internal static class DepthFormats
{
    /// <summary>The console's DXGI_FORMAT_D16_UNORM_S8_UINT, R16_UNORM_X8_TYPELESS and X16_TYPELESS_G8_UINT.</summary>
    public const int ConsoleD16S8 = 118;
    public const int ConsoleR16X8 = 119;
    public const int ConsoleX16G8 = 120;

    /// <summary>DXGI_FORMAT_R32G8X24_TYPELESS, the typeless family of D32_FLOAT_S8X24_UINT.</summary>
    public const int R32G8X24Typeless = 19;

    /// <summary>DXGI_FORMAT_D32_FLOAT_S8X24_UINT: a desktop depth format with stencil that holds 16-bit depth exactly.</summary>
    public const int D32S8 = 20;

    /// <summary>DXGI_FORMAT_R32_FLOAT_X8X24_TYPELESS and X32_TYPELESS_G8X24_UINT: SRVs of the depth and stencil planes.</summary>
    public const int R32X8X24 = 21;
    public const int X32G8X24 = 22;

    /// <summary>The desktop format for the console DSVFormat or clear-value format <paramref name="format"/>; others pass unchanged.</summary>
    public static int Desktop(int format)
    {
        if (format == ConsoleD16S8)
        {
            return D32S8;
        }

        return format;
    }

    /// <summary>The desktop format of a resource created in <paramref name="format"/>.</summary>
    public static int Resource(int format)
    {
        if (IsConsoleDepth(format))
        {
            return R32G8X24Typeless;
        }

        return format;
    }

    /// <summary>The desktop format of an SRV in <paramref name="format"/>: the console depth format reads the depth plane.</summary>
    public static int ShaderView(int format)
    {
        if (format == ConsoleD16S8 || format == ConsoleR16X8)
        {
            return R32X8X24;
        }

        if (format == ConsoleX16G8)
        {
            return X32G8X24;
        }

        return format;
    }

    public static bool IsConsoleDepth(int format)
    {
        return format is >= ConsoleD16S8 and <= ConsoleX16G8;
    }
}
