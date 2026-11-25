// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Formats.Common;
using FiveStar.Formats.Compression;
using FiveStar.Formats.Ttpf;
using FiveStar.Graphics.Rendering;
using Vortice.DXGI;

namespace FiveStar.Cli.Graphics;

/// <summary>
/// Finds a texture by its 64-bit name hash in the texture page files of a dump and reads mip 0. sRGB formats are read
/// as their UNORM twins, so the stored values reach the UNORM target (and the PNG) unchanged.
/// </summary>
public static class ModelTexture
{
    private const byte White = 0xFF;
    private const int RgbaBytes = 4;

    private const int RedShift = 16;
    private const int GreenShift = 8;

    /// <summary>A 1x1 opaque texture of one colour given as 0xRRGGBB, for models whose texture is not found.</summary>
    public static TextureData Solid(int rgb)
    {
        byte[] texel = [(byte)(rgb >> RedShift), (byte)(rgb >> GreenShift), (byte)rgb, White];

        return new TextureData(Format.R8G8B8A8_UNorm, 1, 1, RgbaBytes, 1, texel);
    }

    /// <summary>The page file and index holding the texture, or null.</summary>
    public static (string Path, int Index)? Find(string dump, ulong hash)
    {
        foreach (var path in Directory.EnumerateFiles(dump, "*.ttpf", SearchOption.AllDirectories))
        {
            using var source = new MappedFileSource(path);
            var file = new TexturePageFile(source, new ZlibInflater());
            var index = IndexOf(file.Hashes, hash);

            if (index >= 0)
            {
                return (path, index);
            }
        }

        return null;
    }

    public static TextureData Load(string path, int index)
    {
        var texture = SampleTexture.Load(path, index);

        return texture with { Format = Linear(texture.Format) };
    }

    private static int IndexOf(IReadOnlyList<ulong> hashes, ulong hash)
    {
        for (var index = 0; index < hashes.Count; index++)
        {
            if (hashes[index] == hash)
            {
                return index;
            }
        }

        return -1;
    }

    private static Format Linear(Format format)
    {
        switch (format)
        {
            case Format.BC1_UNorm_SRgb:
                return Format.BC1_UNorm;
            case Format.BC2_UNorm_SRgb:
                return Format.BC2_UNorm;
            case Format.BC3_UNorm_SRgb:
                return Format.BC3_UNorm;
            case Format.BC7_UNorm_SRgb:
                return Format.BC7_UNorm;
            case Format.R8G8B8A8_UNorm_SRgb:
                return Format.R8G8B8A8_UNorm;
            case Format.B8G8R8A8_UNorm_SRgb:
                return Format.B8G8R8A8_UNorm;
            default:
                return format;
        }
    }
}
