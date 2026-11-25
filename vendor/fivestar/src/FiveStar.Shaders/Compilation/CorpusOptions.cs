// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Translation;

namespace FiveStar.Shaders.Compilation;

/// <summary>
/// A corpus run: where the HLSL goes, which stages to translate (all when null), how many programs run at once, and
/// the results.json of an earlier run whose final programs (ok, ok-msfb, skip-stage) are kept instead of re-run.
/// </summary>
public sealed record CorpusOptions(string OutputFolder, IReadOnlySet<ShaderStage>? Stages, int Parallelism, string? RetryResults)
{
    /// <summary>The default translator choices: structured control flow, pinned and emulated wave sizes.</summary>
    public static TranslationOptions DefaultTranslation { get; } = new(StructuredControlFlow: true, PinWaveSize: true, EmulateWaveSize: true);

    /// <summary>The translator choices (<see cref="DefaultTranslation"/> unless set).</summary>
    public TranslationOptions Translation { get; init; } = DefaultTranslation;

    /// <summary>
    /// Whether a mesh program that compiles also gets the compute + vertex-pulling fallback, for GPUs without mesh
    /// shaders (a program over the PC output limits always gets it).
    /// </summary>
    public bool MeshFallbacks { get; init; }

    /// <summary>
    /// The GPUs the run is for; null compiles every form. With them, of a program with a wave variant (a pinned compute
    /// program, a mesh program with a superstep form) only the forms one of these GPUs picks are compiled.
    /// </summary>
    public IReadOnlyList<WaveRange>? Waves { get; init; }
}
