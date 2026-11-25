// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Cache;

namespace FiveStar.Cli.Graphics;

/// <summary>
/// A vertex + pixel pipeline of the console's pipeline cache: its record, program names, fixed-function state and
/// vertex input layout.
/// </summary>
public sealed record SamplePipeline(
    GraphicsPipelineRecord Record,
    string VertexProgram,
    string PixelProgram,
    FixedFunctionState State,
    IReadOnlyList<InputElement> InputLayout)
{
    /// <summary>The immediate-mode textured unlit pass: VS_TransformUnlit_IM + PS_TexturedUnlit_IM.</summary>
    public const string UnlitVertex = "_VS_TransformUnlit_IM_";

    public const string UnlitPixel = "_PS_TexturedUnlit_IM_";

    /// <summary>The first pipeline whose vertex and pixel programs contain the given name parts.</summary>
    public static SamplePipeline Find(PipelineCache cache, CacheProgramMap programs, string vertexPart, string pixelPart)
    {
        foreach (var record in cache.Graphics)
        {
            var vertex = Program(programs, record, PipelineSlot.Vertex, vertexPart);
            var pixel = Program(programs, record, PipelineSlot.Pixel, pixelPart);
            var registers = cache.State(record);

            if (vertex is null || pixel is null || registers is null)
            {
                continue;
            }

            var layout = InputLayoutParser.Parse(cache.Meta(record.InputLayoutMeta).Span);

            return new SamplePipeline(record, vertex, pixel, FixedFunctionDecoder.Decode(registers), layout);
        }

        throw new InvalidDataException($"no pipeline in the cache runs *{vertexPart}* and *{pixelPart}*");
    }

    private static string? Program(CacheProgramMap programs, GraphicsPipelineRecord record, PipelineSlot slot, string part)
    {
        var link = record.Stage(slot);

        if (!link.HasValue)
        {
            return null;
        }

        return programs.Programs(link.Value.Shader).FirstOrDefault(name => name.Contains(part, StringComparison.Ordinal));
    }
}
