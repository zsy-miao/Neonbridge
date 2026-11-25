// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Collections.Concurrent;
using FiveStar.Shaders.Compilation;
using FiveStar.Shaders.Containers;

namespace FiveStar.Cli.Shader;

/// <summary>
/// <c>fivestar shader rect-gs</c>: compiles the rectangle-list geometry shader (<see cref="RectangleGeometryShader"/>)
/// of every compiled vertex program of a translate run into <c>&lt;out&gt;/rect-gs/</c>, one file per distinct output
/// signature. The loader picks the file by the same hash when the game draws a rectangle list.
/// </summary>
internal static class RectangleShaderCommand
{
    private const int Success = 0;
    private const int Failure = 1;
    private const int OnePath = 1;
    private const string VertexStage = "vs";
    private const string GeometryProfile = "gs";
    private const string HlslExtension = ".hlsl";

    public static int Run(IReadOnlyList<string> args)
    {
        var arguments = new CommandArguments(args, new HashSet<string>());

        if (arguments.Positional.Count != OnePath)
        {
            throw new ArgumentException("rect-gs <translate-out> [--jobs N] [--dxc <path>] [--backend hlsl|dxil|dxil-only]");
        }

        var folder = arguments.Positional[0];
        var vertexPrograms = ManifestBuilder.Load(folder).Programs
            .Where(program => program.Stage == VertexStage && program.Dxil is not null)
            .Select(program => Path.Combine(folder, program.Dxil!))
            .ToList();
        var (sources, unsupported) = Sources(vertexPrograms);

        var output = Path.Combine(folder, RectangleGeometryShader.Folder);
        Directory.CreateDirectory(output);
        var failed = Compile(ShaderTools.Backend(arguments, retry: false), output, sources, arguments.IntegerOption("jobs") ?? 1);
        Console.WriteLine($"{vertexPrograms.Count} vertex programs, {sources.Count} geometry shaders in {output}, {failed} failed, {unsupported} signatures not supported");

        if (failed > 0)
        {
            return Failure;
        }

        return Success;
    }

    /// <summary>The distinct sources of the programs, and how many programs have a signature the generator refuses.</summary>
    private static (List<string> Sources, int Unsupported) Sources(IReadOnlyList<string> programs)
    {
        var sources = new HashSet<string>(StringComparer.Ordinal);
        var unsupported = 0;

        foreach (var path in programs)
        {
            try
            {
                var outputs = PsvSignatureParser.Parse(ProgramChunks.Psv(File.ReadAllBytes(path)).Span).Outputs;
                sources.Add(RectangleGeometryShader.Source(outputs));
            }
            catch (NotSupportedException)
            {
                unsupported++;
            }
        }

        return (sources.ToList(), unsupported);
    }

    /// <summary>Compiles the sources whose DXIL is not there yet, N at once; returns the number that failed.</summary>
    private static int Compile(IShaderCompiler compiler, string output, IReadOnlyList<string> sources, int jobs)
    {
        var failures = new ConcurrentBag<string>();
        var options = new ParallelOptions { MaxDegreeOfParallelism = jobs };

        Parallel.ForEach(sources, options, source =>
        {
            var dxil = Path.Combine(output, RectangleGeometryShader.FileName(source));

            if (File.Exists(dxil))
            {
                return;
            }

            var hlsl = Path.ChangeExtension(dxil, HlslExtension);
            File.WriteAllText(hlsl, source);
            var outcome = compiler.Compile(new CompileRequest(hlsl, source, GeometryProfile, DisableOptimizations: false), compiler.Options.CompileTimeout);

            if (!outcome.Succeeded)
            {
                failures.Add($"{Path.GetFileName(hlsl)}: {DxcCompiler.FirstError(outcome.Error)}");
            }
        });

        foreach (var failure in failures)
        {
            Console.Error.WriteLine(failure);
        }

        return failures.Count;
    }
}
