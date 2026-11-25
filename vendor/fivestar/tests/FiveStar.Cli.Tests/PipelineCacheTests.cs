// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Cli.Shader;
using FiveStar.Cli.Tests.Support;
using FiveStar.Shaders.Cache;
using Xunit;

namespace FiveStar.Cli.Tests;

/// <summary>The driver pipeline cache of the user's shaders.rpf reads completely and links to the extracted programs.</summary>
public sealed class PipelineCacheTests
{
    [Fact]
    public void Shader_metadata_decodes_user_data_mappings_and_declarations()
    {
        // Graphics header, then: t13 at dword 8 of the table of root parameter 0 (s8), an 8-dword image descriptor;
        // t1 as root descriptor of parameter 2 (s[10:11]); s3 as static sampler 11; a used Texture2D declaration of t13.
        uint[] words =
        [
            2, 0x8C, 0x100, 0x440,
            0x41, 0x300, 8, 13, 4, 0x00_08_08, 0xFFFFFFFF, 0,
            0x41, 0x100, 0, 1, 0, 0x02_0A_02, 0xFFFFFFFF, 0,
            0x41, 0x401, 0, 3, 0, 0x0B_00_04, 0xFFFFFFFF, 0,
            0x42, 0, 0x0002_00_01, 13, 0, 0, 0xFFFFFFFF, 0,
            0x43, 0, 0, 0, 0,
        ];
        var bytes = words.SelectMany(BitConverter.GetBytes).ToArray();

        var meta = ShaderMetaParser.Parse(bytes)!;

        Assert.Equal(
            new[]
            {
                new UserDataEntry(UserDataKind.TableEntry, BindingClass.ShaderResource, 8, 13, 4, 8, 8, 0),
                new UserDataEntry(UserDataKind.RootDescriptor, BindingClass.ShaderResource, 0, 1, 0, 2, 10, 2),
                new UserDataEntry(UserDataKind.StaticSampler, BindingClass.Sampler, 0, 3, 0, 4, 0, 11),
            },
            meta.UserData);
        Assert.Equal(new ResourceEntry(true, BindingClass.ShaderResource, 2, 13, 0), Assert.Single(meta.Resources));
    }

    [DumpFact]
    public void The_cache_in_shaders_rpf_reads_completely_and_its_pipelines_resolve_to_programs()
    {
        var cache = PipelineCacheSource.Load(Path.Combine(DumpFactAttribute.Folder!, "shaders.rpf"));

        Assert.Equal("930.048.020", cache.Header.DriverVersion);
        Assert.Equal((25471, 2693, 54576, 21522), (cache.Header.GraphicsCount, cache.Header.ComputeCount, cache.Header.MetaCount, cache.Header.ShaderCount));
        Assert.Equal(275540780, cache.Length);
        Assert.Equal(13385, cache.Graphics.Count(pipeline => pipeline.Stage(PipelineSlot.Vertex).HasValue && pipeline.Stage(PipelineSlot.Pixel).HasValue));
        Assert.Equal(12062, cache.Graphics.Count(pipeline => pipeline.Stage(PipelineSlot.Mesh).HasValue));
        Assert.All(cache.Graphics, pipeline => InputLayoutParser.Parse(cache.Meta(pipeline.InputLayoutMeta).Span));

        // Every shader's bindings decode; the output formats are stored per pipeline.
        Assert.All(cache.Graphics.SelectMany(pipeline => pipeline.Stages).Where(link => link.Meta >= 0), link => Assert.NotNull(ShaderMetaParser.Parse(cache.Meta(link.Meta).Span)));
        var targets = cache.Graphics.Select(pipeline => PipelineTargetsParser.Parse(cache.Meta(pipeline.InputLayoutMeta).Span)).ToList();
        Assert.Equal((7079, 24905), (targets.Count(target => target.DepthStencilFormat == 0), targets.Sum(target => target.RenderTargetFormats.Count(format => format == 28))));

        if (DumpFactAttribute.ShaderFolder is null)
        {
            return;
        }

        var programs = CacheProgramMap.Build(cache, ProgramIndex.Load(DumpFactAttribute.ShaderFolder));
        var resolved = cache.Graphics.Where(programs.Resolves).ToList();

        Assert.Equal(21502, programs.Matched);
        Assert.Equal(13362, resolved.Count(pipeline => pipeline.Stage(PipelineSlot.Vertex).HasValue && pipeline.Stage(PipelineSlot.Pixel).HasValue));
        Assert.Equal(10726, resolved.Count(pipeline => pipeline.Stage(PipelineSlot.Mesh).HasValue && pipeline.Stage(PipelineSlot.Pixel).HasValue));
    }
}
