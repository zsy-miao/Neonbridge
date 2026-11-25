// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Formats.Common;
using FiveStar.Formats.Compression;
using FiveStar.Formats.Ttpf;
using FiveStar.Graphics.Rendering;
using Vortice.DXGI;

namespace FiveStar.Cli.Graphics;

/// <summary>Reads mip 0 of one texture of a streamed texture page file (<c>.ttpf</c>) for upload.</summary>
public static class SampleTexture
{
    public static TextureData Load(string path, int index)
    {
        using var source = new MappedFileSource(path);
        var file = new TexturePageFile(source, new ZlibInflater());

        if (index < 0 || index >= file.Descriptors.Count)
        {
            throw new ArgumentException($"{path} has {file.Descriptors.Count} textures, no #{index}");
        }

        var descriptor = file.Descriptors[index];
        var (status, data, layout) = new TextureExtractor(file).ReadMip0(descriptor);

        if (data is null || layout is null)
        {
            throw new InvalidDataException($"{path} #{index}: {status.Name}");
        }

        if (layout.Images != 1 || layout.Depth != 1)
        {
            throw new InvalidDataException($"{path} #{index} is a {layout.Kind}, not one 2D image");
        }

        var rowBytes = layout.BlockWidth * layout.Format.BytesPerElement;

        return new TextureData((Format)descriptor.Format, descriptor.Width, descriptor.Height, rowBytes, layout.BlockHeight, data);
    }
}
