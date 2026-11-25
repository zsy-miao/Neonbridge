// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Formats.Common;
using FiveStar.Formats.Compression;
using FiveStar.Formats.Rpf;
using FiveStar.Shaders.Cache;

namespace FiveStar.Loader.Tests.Support;

/// <summary>The console pipeline cache inside the dump's shaders.rpf (FIVESTAR_DUMP).</summary>
internal static class DumpPipelineCache
{
    private const string Archive = "shaders.rpf";

    public static PipelineCache Read()
    {
        using var source = new MappedFileSource(Path.Combine(DumpFactAttribute.Folder!, Archive));
        var reader = new ChunkRunReader(source, new ZlibStreamProbe(source, new ZlibInflater()), new ZlibInflater());
        var start = reader.Find(PipelineCacheHeader.Magic);
        var header = PipelineCacheHeader.Parse(reader.Read(start, PipelineCacheHeader.Size).Data);

        return PipelineCache.Read(reader.Read(start, header.TotalSize).Data);
    }
}
