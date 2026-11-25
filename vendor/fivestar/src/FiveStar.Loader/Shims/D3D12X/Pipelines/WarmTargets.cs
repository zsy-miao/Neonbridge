// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Cache;

namespace FiveStar.Loader.Shims.D3D12X.Pipelines;

/// <summary>
/// The pipelines the game creates from its pipeline cache (slots 57 and 67), as the descriptions those slots read:
/// compute first, the largest code first (the driver needs minutes for the largest), then graphics in record order.
/// </summary>
internal static class WarmTargets
{
    /// <summary>Every <paramref name="step"/>-th compute and graphics record, those with a code block cut off left out.</summary>
    public static List<WarmTarget> Read(PipelineCache cache, int step)
    {
        var targets = new List<WarmTarget>();
        var compute = cache.Compute.Where((_, index) => index % step == 0)
            .Select(record => (Record: record, Code: cache.Shader(record.Shader)))
            .Where(entry => entry.Code.HasValue)
            .OrderByDescending(entry => entry.Code!.Value.Length);

        foreach (var (record, code) in compute)
        {
            var description = new ComputePipelineDescription(0, PipelineDescReader.CacheCode(code!.Value.Span));
            targets.Add(new WarmTarget(record.Index, null, description));
        }

        foreach (var record in cache.Graphics.Where((_, index) => index % step == 0))
        {
            var description = Graphics(cache, record);

            if (description is not null)
            {
                targets.Add(new WarmTarget(record.Index, description, null));
            }
        }

        return targets;
    }

    private static GraphicsPipelineDescription? Graphics(PipelineCache cache, GraphicsPipelineRecord record)
    {
        var shaders = new List<ShaderSource>();

        foreach (var link in record.Stages.OrderBy(link => link.Slot))
        {
            var code = cache.Shader(link.Shader);

            if (!code.HasValue)
            {
                return null;
            }

            shaders.Add(PipelineDescReader.CacheCode(code.Value.Span));
        }

        try
        {
            return PipelineDescReader.CachedGraphics(cache.Meta(record.StateMeta).ToArray(), cache.Meta(record.InputLayoutMeta).ToArray(), shaders, 0);
        }
        catch (NotSupportedException)
        {
            return null;
        }
    }
}
