// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Formats.Common;
using FiveStar.Formats.Compression;
using FiveStar.Formats.Rpf;
using FiveStar.Shaders.Cache;

namespace FiveStar.Cli.Shader;

/// <summary>Loads the driver pipeline cache from shaders.rpf (found by its magic) or from a blob file.</summary>
public static class PipelineCacheSource
{
    /// <summary>Reads the cache of an archive or a blob that starts with the cache magic.</summary>
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

        return FromArchive(path);
    }

    private static PipelineCache FromArchive(string path)
    {
        using var source = new MappedFileSource(path);
        var inflater = new ZlibInflater();
        var reader = new ChunkRunReader(source, new ZlibStreamProbe(source, new ZlibInflater()), inflater);
        var start = reader.Find(PipelineCacheHeader.Magic);

        if (start < 0)
        {
            throw new InvalidDataException($"{path} holds no pipeline cache");
        }

        var header = PipelineCacheHeader.Parse(reader.Read(start, PipelineCacheHeader.Size).Data);

        return PipelineCache.Read(reader.Read(start, header.TotalSize).Data);
    }
}
