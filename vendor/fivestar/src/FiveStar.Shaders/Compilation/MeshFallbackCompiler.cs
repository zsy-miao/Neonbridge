// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;
using FiveStar.Shaders.Translation;

namespace FiveStar.Shaders.Compilation;

/// <summary>Writes and compiles the compute + vertex-pulling replacement of a mesh shader dxc rejected.</summary>
public sealed class MeshFallbackCompiler
{
    /// <summary>The file suffix of the fallback compute shader.</summary>
    public const string ComputeSuffix = ".msfb_cs.hlsl";

    /// <summary>The file suffix of the fallback vertex shader.</summary>
    public const string VertexSuffix = ".msfb_vs.hlsl";

    private const string EntryAttribute = "[numthreads(";

    private readonly IShaderCompiler _compiler;

    public MeshFallbackCompiler(IShaderCompiler compiler)
    {
        _compiler = compiler;
    }

    /// <summary>The suffix of the fallback compute shader pinned to <paramref name="lanes"/>-lane waves.</summary>
    public static string PinnedComputeSuffix(int lanes)
    {
        return string.Create(CultureInfo.InvariantCulture, $".msfb_cs.w{lanes}.hlsl");
    }

    /// <summary>Null when both shaders compile; otherwise why the fallback failed.</summary>
    public string? Compile(string meshPath, string meshHlsl, bool large)
    {
        string compute;
        string vertex;

        try
        {
            (compute, vertex) = MeshComputeFallback.Rewrite(meshHlsl);
        }
        catch (UnsupportedProgramException exception)
        {
            return exception.Message;
        }

        var stem = Path.ChangeExtension(meshPath, null);

        return CompileParts(stem, large, [(compute, "cs", ComputeSuffix), (vertex, "vs", VertexSuffix)]);
    }

    /// <summary>
    /// The fallback compute shader pinned with <c>[WaveSize(lanes)]</c>: with hardware waves as wide as the console wave
    /// it gives console results whatever the program's floor, on GPUs that offer that wave size. Its vertex shader is the
    /// one <see cref="Compile"/> writes. Null when it compiles; otherwise why not.
    /// </summary>
    public string? CompilePinned(string meshPath, string meshHlsl, bool large, int lanes)
    {
        string compute;

        try
        {
            compute = MeshComputeFallback.Rewrite(meshHlsl).Compute;
        }
        catch (UnsupportedProgramException exception)
        {
            return exception.Message;
        }

        var entry = compute.IndexOf(EntryAttribute, StringComparison.Ordinal);
        var pinned = compute.Insert(entry, string.Create(CultureInfo.InvariantCulture, $"[WaveSize({lanes})]\n"));
        var stem = Path.ChangeExtension(meshPath, null);

        return CompileParts(stem, large, [(pinned, "cs", PinnedComputeSuffix(lanes))]);
    }

    private string? CompileParts(string stem, bool large, IReadOnlyList<(string Source, string Profile, string Suffix)> parts)
    {
        foreach (var (source, profile, suffix) in parts)
        {
            var path = stem + suffix;
            File.WriteAllText(path, source);
            var outcome = _compiler.Compile(new CompileRequest(path, source, profile, large), _compiler.Options.FallbackTimeout);

            if (outcome.TimedOut)
            {
                return "dxc timeout";
            }

            if (!outcome.Succeeded)
            {
                return DxcCompiler.FirstError(outcome.Error);
            }
        }

        return null;
    }
}
