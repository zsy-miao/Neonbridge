// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Graphics.Programs;
using FiveStar.Shaders.Cache;

namespace FiveStar.Cli.Shader;

/// <summary>
/// The distinct VS+PS and MS+PS pairs of the console pipeline cache whose programs compiled, in name order. A cache
/// shader can match several extracted programs (same code); the first compiled one stands for it.
/// </summary>
internal static class LinkPairs
{
    private const string Pixel = "ps";
    private const int PairStages = 2;

    public static List<LinkPair> Find(PipelineCache cache, CacheProgramMap programs, IReadOnlyDictionary<string, ProgramVariants> compiled)
    {
        var pairs = new Dictionary<string, LinkPair>(StringComparer.Ordinal);

        foreach (var pipeline in cache.Graphics.Where(pipeline => pipeline.Stages.Count == PairStages))
        {
            var pixel = pipeline.Stage(PipelineSlot.Pixel);
            var producer = pipeline.Stage(PipelineSlot.Vertex) ?? pipeline.Stage(PipelineSlot.Mesh);

            if (!pixel.HasValue || !producer.HasValue)
            {
                continue;
            }

            var consumer = Compiled(programs.Programs(pixel.Value.Shader), compiled);
            var source = Compiled(programs.Programs(producer.Value.Shader), compiled);

            if (consumer is null || source is null || consumer.Stage != Pixel || consumer.Dxil is null)
            {
                continue;
            }

            var pair = new LinkPair(source, consumer);
            pairs.TryAdd(pair.Name, pair);
        }

        return pairs.Values.OrderBy(pair => pair.Name, StringComparer.Ordinal).ToList();
    }

    private static ProgramVariants? Compiled(IReadOnlyList<string> names, IReadOnlyDictionary<string, ProgramVariants> compiled)
    {
        foreach (var name in names)
        {
            if (compiled.TryGetValue(name, out var program))
            {
                return program;
            }
        }

        return null;
    }
}
