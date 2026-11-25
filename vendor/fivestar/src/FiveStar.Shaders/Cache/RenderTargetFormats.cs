// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Cache;

/// <summary>
/// Guesses a render target's DXGI_FORMAT from its SX_PS_DOWNCONVERT class and SPI_SHADER_COL_FORMAT export format.
/// Enum values: SX_DOWNCONVERT_FORMAT and SPI_SHADER_EX_FORMAT of AMD's gfx9+ register headers; DXGI_FORMAT of dxgiformat.h.
/// </summary>
public static class RenderTargetFormats
{
    /// <summary>DXGI_FORMAT_UNKNOWN: no guess.</summary>
    public const int Unknown = 0;

    // SX_RT_EXPORT_* classes.
    private const int NoConversion = 0;
    private const int Export32R = 1;
    private const int ExportR11G11B10 = 3;
    private const int ExportR10G10B10A2 = 4;
    private const int ExportRgba8 = 5;
    private const int ExportB5G6R5 = 6;
    private const int ExportB5G5R5A1 = 7;
    private const int ExportB4G4R4A4 = 8;
    private const int ExportRg16 = 9;
    private const int ExportAr16 = 10;
    private const int ExportRgb9E5 = 11;

    // SPI_SHADER_* export formats.
    private const int Spi32R = 1;
    private const int Spi32Gr = 2;
    private const int SpiFp16 = 4;
    private const int SpiUnorm16 = 5;
    private const int SpiSnorm16 = 6;
    private const int SpiUint16 = 7;
    private const int SpiSint16 = 8;
    private const int Spi32Abgr = 9;

    // DXGI_FORMAT values.
    private const int R32G32B32A32Float = 2;
    private const int R16G16B16A16Float = 10;
    private const int R16G16B16A16Unorm = 11;
    private const int R16G16B16A16Uint = 12;
    private const int R16G16B16A16Snorm = 13;
    private const int R16G16B16A16Sint = 14;
    private const int R32G32Float = 16;
    private const int R10G10B10A2Unorm = 24;
    private const int R11G11B10Float = 26;
    private const int R8G8B8A8Unorm = 28;
    private const int R8G8B8A8Uint = 30;
    private const int R16G16Float = 34;
    private const int R16G16Unorm = 35;
    private const int R32Float = 41;
    private const int R16Float = 54;
    private const int R16Unorm = 56;
    private const int R16Uint = 57;
    private const int R16Snorm = 59;
    private const int R9G9B9E5SharedExp = 67;
    private const int B5G6R5Unorm = 85;
    private const int B5G5R5A1Unorm = 86;
    private const int B4G4R4A4Unorm = 115;

    /// <summary>The guessed DXGI_FORMAT of a target, <see cref="Unknown"/> when the pair has no single family.</summary>
    public static int Guess(int downconvert, int export)
    {
        return downconvert switch
        {
            NoConversion => Unconverted(export),
            Export32R => R32Float,
            ExportR11G11B10 => R11G11B10Float,
            ExportR10G10B10A2 => R10G10B10A2Unorm,
            ExportRgba8 => EightBit(export),
            ExportB5G6R5 => B5G6R5Unorm,
            ExportB5G5R5A1 => B5G5R5A1Unorm,
            ExportB4G4R4A4 => B4G4R4A4Unorm,
            ExportRg16 => TwoChannel16(export),
            ExportAr16 => OneChannel16(export),
            ExportRgb9E5 => R9G9B9E5SharedExp,
            _ => Unknown,
        };
    }

    private static int Unconverted(int export)
    {
        return export switch
        {
            SpiFp16 => R16G16B16A16Float,
            SpiUnorm16 => R16G16B16A16Unorm,
            SpiSnorm16 => R16G16B16A16Snorm,
            SpiUint16 => R16G16B16A16Uint,
            SpiSint16 => R16G16B16A16Sint,
            Spi32Abgr => R32G32B32A32Float,
            Spi32Gr => R32G32Float,
            Spi32R => R32Float,
            _ => Unknown,
        };
    }

    private static int EightBit(int export)
    {
        if (export == SpiUint16)
        {
            return R8G8B8A8Uint;
        }

        return R8G8B8A8Unorm;
    }

    private static int TwoChannel16(int export)
    {
        if (export == SpiUnorm16)
        {
            return R16G16Unorm;
        }

        return R16G16Float;
    }

    private static int OneChannel16(int export)
    {
        return export switch
        {
            SpiUnorm16 => R16Unorm,
            SpiSnorm16 => R16Snorm,
            SpiUint16 => R16Uint,
            _ => R16Float,
        };
    }
}
