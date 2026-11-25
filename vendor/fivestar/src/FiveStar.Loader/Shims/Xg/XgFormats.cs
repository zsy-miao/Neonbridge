// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.Xg;

/// <summary>
/// Element sizes of the DXGI formats by enumeration range (dxgiformat.h). Depth formats with stencil get a separate
/// one-byte stencil plane, as the GPU stores them; format 118 is the console's D16_UNORM_S8_UINT, which the game
/// treats like D32_FLOAT_S8X24_UINT (20). Unknown formats are taken as 4 bytes.
/// </summary>
internal static class XgFormats
{
    private const int Bytes1 = 1;
    private const int Bytes2 = 2;
    private const int Bytes4 = 4;
    private const int Bytes8 = 8;
    private const int Bytes12 = 12;
    private const int Bytes16 = 16;
    private const int Pixel = 1;
    private const int Block = 4;

    // DXGI_FORMAT ranges, first and last member of each run with one element size.
    private const uint Rgba32First = 1;
    private const uint Rgba32Last = 4;
    private const uint Rgb32Last = 8;
    private const uint Rg32Last = 18;
    private const uint R32G8X24First = 19;
    private const uint R32G8X24Last = 22;
    private const uint R32Last = 43;
    private const uint R24G8First = 44;
    private const uint R24G8Last = 47;
    private const uint R16Last = 59;
    private const uint R8Last = 66;
    private const uint SharedExponentLast = 69;
    private const uint Bc1First = 70;
    private const uint Bc1Last = 72;
    private const uint Bc3Last = 78;
    private const uint Bc4Last = 81;
    private const uint Bc5Last = 84;
    private const uint B5G5R5A1Last = 86;
    private const uint Bgra8Last = 93;
    private const uint Bc6First = 94;
    private const uint Bc7Last = 99;
    private const uint B4G4R4A4 = 115;
    private const uint D16S8 = 118;

    private static readonly XgElementFormat Default = new(Bytes4, Pixel, false);

    [HotPath]
    public static XgElementFormat Find(uint dxgi)
    {
        return dxgi switch
        {
            < Rgba32First => Default,
            <= Rgba32Last and <= Rgba32Last => new XgElementFormat(Bytes16, Pixel, false),
            <= Rgb32Last => new XgElementFormat(Bytes12, Pixel, false),
            <= Rg32Last => new XgElementFormat(Bytes8, Pixel, false),
            >= R32G8X24First and <= R32G8X24Last => new XgElementFormat(Bytes4, Pixel, true),
            <= R32Last => Default,
            >= R24G8First and <= R24G8Last => new XgElementFormat(Bytes4, Pixel, true),
            <= R16Last => new XgElementFormat(Bytes2, Pixel, false),
            <= R8Last => new XgElementFormat(Bytes1, Pixel, false),
            <= SharedExponentLast => Default,
            >= Bc1First and <= Bc1Last => new XgElementFormat(Bytes8, Block, false),
            <= Bc3Last => new XgElementFormat(Bytes16, Block, false),
            <= Bc4Last => new XgElementFormat(Bytes8, Block, false),
            <= Bc5Last => new XgElementFormat(Bytes16, Block, false),
            <= B5G5R5A1Last => new XgElementFormat(Bytes2, Pixel, false),
            <= Bgra8Last => Default,
            >= Bc6First and <= Bc7Last => new XgElementFormat(Bytes16, Block, false),
            B4G4R4A4 => new XgElementFormat(Bytes2, Pixel, false),
            D16S8 => new XgElementFormat(Bytes2, Pixel, true),
            _ => Default,
        };
    }
}
