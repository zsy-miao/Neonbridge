// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Cache;

/// <summary>The extracted programs of every cached shader, found by the hash prefix of their code.</summary>
public sealed class CacheProgramMap
{
    private readonly IReadOnlyList<string>?[] _programs;

    private CacheProgramMap(IReadOnlyList<string>?[] programs)
    {
        _programs = programs;
    }

    /// <summary>Gets the number of shaders whose bytes are in the cache blob.</summary>
    public int Available => _programs.Count(names => names is not null);

    /// <summary>Gets the number of shaders with at least one extracted program.</summary>
    public int Matched => _programs.Count(names => names is not null && names.Count > 0);

    /// <summary>Looks up every shader of the cache in the index.</summary>
    public static CacheProgramMap Build(PipelineCache cache, ProgramIndex index)
    {
        var programs = new IReadOnlyList<string>?[cache.Header.ShaderCount];

        for (var shader = 0; shader < programs.Length; shader++)
        {
            var code = cache.Shader(shader);

            if (code.HasValue)
            {
                programs[shader] = index.Find(code.Value.Span);
            }
        }

        return new CacheProgramMap(programs);
    }

    /// <summary>The program names of a shader; empty when none matches or its bytes are missing.</summary>
    public IReadOnlyList<string> Programs(int shader)
    {
        return _programs[shader] ?? [];
    }

    /// <summary>Whether every used slot of the pipeline has a program.</summary>
    public bool Resolves(GraphicsPipelineRecord pipeline)
    {
        return pipeline.Stages.All(link => Programs(link.Shader).Count > 0);
    }
}
