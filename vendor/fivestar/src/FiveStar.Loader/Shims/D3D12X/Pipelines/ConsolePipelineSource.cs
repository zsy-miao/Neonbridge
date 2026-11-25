// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Formats.Common;
using FiveStar.Formats.Compression;
using FiveStar.Formats.Rpf;
using FiveStar.Shaders.Cache;

namespace FiveStar.Loader.Shims.D3D12X.Pipelines;

/// <summary>
/// The game's <c>.d3d12PipelineCache</c> (docs/pipeline-cache.md): from shaders.rpf, found by its magic, or from a file
/// that holds the blob alone.
/// </summary>
internal static class ConsolePipelineSource
{
    public static PipelineCache Load(string path)
    {
        var head = new byte[PipelineCacheHeader.Size];

        using (var file = File.OpenRead(path))
        {
            file.ReadAtLeast(head, head.Length, throwOnEndOfStream: false);
        }

        if (PipelineCacheHeader.HasMagic(head))
        {
            return PipelineCache.Read(File.ReadAllBytes(path));
        }

        using var source = new MappedFileSource(path);
        var reader = new ChunkRunReader(source, new ZlibStreamProbe(source, new ZlibInflater()), new ZlibInflater());
        var start = reader.Find(PipelineCacheHeader.Magic);

        if (start < 0)
        {
            throw new InvalidDataException($"{path} holds no pipeline cache");
        }

        var header = PipelineCacheHeader.Parse(reader.Read(start, PipelineCacheHeader.Size).Data);

        return PipelineCache.Read(reader.Read(start, header.TotalSize).Data);
    }
}
