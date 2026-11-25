// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Graphics.Programs;
using FiveStar.Shaders.Analysis;
using FiveStar.Shaders.Compilation;

namespace FiveStar.Cli.Shader;

/// <summary>
/// Builds the <see cref="ShaderManifest"/> of a translate run from results.json, the compiled files and the wave facts
/// of each program's HLSL: every <c>ok</c> and <c>ok-msfb</c> program with its native DXIL, wave32 variant and mesh
/// fallback where they compiled.
/// </summary>
internal static class ManifestBuilder
{
    private const string Compute = "cs";

    /// <summary>The manifest of a translate run's folder, written as <see cref="ShaderManifest.FileName"/>; returns it.</summary>
    public static ShaderManifest Write(string folder)
    {
        var manifest = Build(folder);
        manifest.Write(Path.Combine(folder, ShaderManifest.FileName));

        return manifest;
    }

    /// <summary>The manifest of the folder, read when it is newer than results.json and built otherwise.</summary>
    public static ShaderManifest Load(string folder)
    {
        var path = Path.Combine(folder, ShaderManifest.FileName);
        var results = Path.Combine(folder, CorpusTranslator.ResultsName);

        if (File.Exists(path) && File.GetLastWriteTimeUtc(path) >= File.GetLastWriteTimeUtc(results))
        {
            return ShaderManifest.Read(path);
        }

        return Write(folder);
    }

    public static ShaderManifest Build(string folder)
    {
        var programs = ResultsFile.Read(Path.Combine(folder, CorpusTranslator.ResultsName))
            .Where(result => result.Status is ProgramStatus.Ok or ProgramStatus.OkMeshFallback && result.Stage is not null)
            .Select(result => Describe(folder, result))
            .ToList();

        return new ShaderManifest(programs);
    }

    private static ProgramVariants Describe(string folder, ProgramResult result)
    {
        var facts = Facts(Path.Combine(folder, result.Name + ".hlsl"), result.Stage == Compute);
        string? dxil = null;
        string? groupWave = null;
        MeshFallbackFiles? fallback = null;

        if (result.Status == ProgramStatus.Ok)
        {
            dxil = Existing(folder, result.Name + ".dxil");
        }

        var notes = (result.Reason ?? string.Empty).Split(CorpusTranslator.ReasonSeparator);

        if (notes.Contains(CorpusTranslator.GroupWaveReady))
        {
            groupWave = Existing(folder, Path.ChangeExtension(result.Name + CorpusTranslator.GroupWaveSuffix, ".dxil"));
        }

        if (result.Status == ProgramStatus.OkMeshFallback || notes.Contains(CorpusTranslator.MeshFallbackReady))
        {
            fallback = WideFallbacks(folder, result.Name, facts.ProgramWave, notes, Fallback(folder, result.Name));
        }

        return new ProgramVariants(result.Name, result.Stage!, dxil, facts.ProgramWave, facts.Floor, facts.WavePin, groupWave, fallback);
    }

    /// <summary>The fallback with its compute shader pinned to the console wave and its superstep form, where they compiled.</summary>
    private static MeshFallbackFiles? WideFallbacks(string folder, string name, int? wave, string[] notes, MeshFallbackFiles? fallback)
    {
        if (fallback is null)
        {
            return null;
        }

        if (wave is not null && notes.Contains(CorpusTranslator.PinnedFallbackNote(wave.Value) + CorpusTranslator.ReadySuffix))
        {
            var pinned = Existing(folder, Path.ChangeExtension(name + MeshFallbackCompiler.PinnedComputeSuffix(wave.Value), ".dxil"));
            fallback = fallback with { PinnedCompute = pinned, PinnedLanes = wave };
        }

        if (notes.Contains(CorpusTranslator.SuperstepFallbackReady))
        {
            fallback = fallback with { Supersteps = Fallback(folder, Path.ChangeExtension(name + CorpusTranslator.GroupWaveSuffix, null)) };
        }

        return fallback;
    }

    /// <summary>The wave facts: the floor is on the first line; a compute program's pin is above its entry point.</summary>
    private static WaveFacts Facts(string hlslPath, bool compute)
    {
        if (!File.Exists(hlslPath))
        {
            return new WaveFacts(null, null, null);
        }

        if (compute)
        {
            return WaveFacts.Read(File.ReadAllText(hlslPath));
        }

        return WaveFacts.Read(File.ReadLines(hlslPath).FirstOrDefault() ?? string.Empty);
    }

    private static MeshFallbackFiles? Fallback(string folder, string name)
    {
        var compute = Existing(folder, name + ".msfb_cs.dxil");
        var vertex = Existing(folder, name + ".msfb_vs.dxil");

        if (compute is null || vertex is null)
        {
            return null;
        }

        return new MeshFallbackFiles(compute, vertex);
    }

    private static string? Existing(string folder, string file)
    {
        if (!File.Exists(Path.Combine(folder, file)))
        {
            return null;
        }

        return file;
    }
}
