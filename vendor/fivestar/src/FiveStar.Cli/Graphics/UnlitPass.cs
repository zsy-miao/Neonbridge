// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Cli.Shader;
using FiveStar.Shaders.Cache;
using FiveStar.Shaders.Disassembly;

namespace FiveStar.Cli.Graphics;

/// <summary>The immediate-mode unlit pass of the pipeline cache with both programs translated to DXIL.</summary>
public sealed record UnlitPass(SamplePipeline Pipeline, TranslatedShader Vertex, TranslatedShader Pixel)
{
    /// <summary>Loads the cache from <c>&lt;dump&gt;/shaders.rpf</c>, finds the unlit pipeline and translates it.</summary>
    public static UnlitPass Load(string dump, string programFolder, string workFolder, IDisassembler disassembler, string dxc)
    {
        var cache = PipelineCacheSource.Load(Path.Combine(dump, "shaders.rpf"));
        var programs = CacheProgramMap.Build(cache, ProgramIndex.Load(programFolder));
        var pipeline = SamplePipeline.Find(cache, programs, SamplePipeline.UnlitVertex, SamplePipeline.UnlitPixel);
        var shaders = TranslatedShader.Translate([pipeline.VertexProgram, pipeline.PixelProgram], programFolder, workFolder, disassembler, dxc);

        return new UnlitPass(pipeline, shaders[0], shaders[1]);
    }
}
