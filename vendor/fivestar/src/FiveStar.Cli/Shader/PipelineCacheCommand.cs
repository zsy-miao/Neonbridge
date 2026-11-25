// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;
using FiveStar.Shaders.Cache;

namespace FiveStar.Cli.Shader;

/// <summary>
/// <c>fivestar shader psocache</c>: summarises the driver pipeline cache, matches its shaders to extracted FSSM
/// programs and exports the shader code and the pipeline records.
/// </summary>
internal static class PipelineCacheCommand
{
    private const int Success = 0;
    private const int OnePath = 1;
    private const string ProgramFolderVariable = "FIVESTAR_SHADER_DIR";
    private const string CodeFolder = "code";
    private const string RecordsFile = "pipelines.json";
    private const int TopValues = 8;

    public static int Run(IReadOnlyList<string> args)
    {
        var arguments = new CommandArguments(args, new HashSet<string>());

        if (arguments.Positional.Count != OnePath)
        {
            throw new ArgumentException("psocache <shaders.rpf | cache blob> [--programs <fssm folder>] [--out <dir>]");
        }

        var cache = PipelineCacheSource.Load(arguments.Positional[0]);
        PrintHeader(cache);
        PrintPipelines(cache);

        var folder = arguments.Option("programs") ?? Environment.GetEnvironmentVariable(ProgramFolderVariable);
        var programs = MatchPrograms(cache, folder);
        var output = arguments.Option("out");

        if (output is not null)
        {
            Export(cache, programs, output);
        }

        return Success;
    }

    private static void PrintHeader(PipelineCache cache)
    {
        var header = cache.Header;
        Console.WriteLine($"cache {header.CacheId}, driver {header.DriverVersion}, {header.TotalSize} bytes");
        Console.WriteLine($"graphics {header.GraphicsCount}, compute {header.ComputeCount}, metadata {header.MetaCount} ({header.MetaBytes} bytes), shaders {header.ShaderCount} ({header.CodeBytes} bytes)");

        if (cache.Length < header.TotalSize)
        {
            Console.WriteLine($"cut off: the blob ends after {cache.Length} bytes");
        }
    }

    private static void PrintPipelines(PipelineCache cache)
    {
        var patterns = cache.Graphics
            .GroupBy(pipeline => string.Join('+', pipeline.Stages.Select(link => link.Slot.ToString().ToLowerInvariant())))
            .OrderByDescending(group => group.Count());
        Console.WriteLine("graphics slots: " + string.Join(", ", patterns.Select(group => $"{group.Key} {group.Count()}")));

        var layouts = cache.Graphics.Select(pipeline => InputLayoutParser.Parse(cache.Meta(pipeline.InputLayoutMeta).Span)).ToList();
        Console.WriteLine("input elements per pipeline: " + Top(layouts.Select(layout => layout.Count)));
        Console.WriteLine("input formats (DXGI): " + Top(layouts.SelectMany(layout => layout.Select(element => element.Format))));

        var kinds = cache.Graphics.SelectMany(pipeline => pipeline.Stages.Select(link => link.Meta))
            .Concat(cache.Compute.Select(pipeline => pipeline.ShaderMeta))
            .Where(meta => meta >= 0)
            .Distinct()
            .Select(meta => ShaderMetaParser.Parse(cache.Meta(meta).Span))
            .SelectMany(meta => meta?.UserData ?? []);
        Console.WriteLine("shader bindings: " + Top(kinds.Select(entry => $"{entry.Kind} {entry.RegisterClass}")));
        PrintStates(cache);
    }

    private static void PrintStates(PipelineCache cache)
    {
        var states = cache.Graphics
            .Select(cache.State)
            .Where(registers => registers is not null)
            .Select(registers => FixedFunctionDecoder.Decode(registers!))
            .ToList();
        Console.WriteLine($"fixed-function state: {states.Count} of {cache.Graphics.Count} decoded");
        Console.WriteLine("render targets per pipeline: " + Top(states.Select(state => state.RenderTargetCount)));
        Console.WriteLine("cull mode (D3D12): " + Top(states.Select(state => state.Rasterizer.CullMode)));
        Console.WriteLine($"blend on target 0: {states.Count(state => state.Blend[0].Enable)}, dual-source {states.Count(state => state.DualSource)}, depth test {states.Count(state => state.DepthStencil.DepthEnable)}, depth clip off {states.Count(state => !state.Rasterizer.DepthClipEnable)}");
    }

    private static CacheProgramMap MatchPrograms(PipelineCache cache, string? folder)
    {
        if (folder is null || !Directory.Exists(folder))
        {
            Console.WriteLine("programs: no FSSM folder (--programs or FIVESTAR_SHADER_DIR), shaders not matched");

            return CacheProgramMap.Build(cache, ProgramIndex.Empty());
        }

        var index = ProgramIndex.Load(folder);
        var map = CacheProgramMap.Build(cache, index);
        Console.WriteLine($"shaders: {map.Available} in the blob, {map.Matched} match one of {index.Count} extracted programs");
        PrintPairs(cache, map);

        return map;
    }

    private static void PrintPairs(PipelineCache cache, CacheProgramMap map)
    {
        var resolved = cache.Graphics.Where(map.Resolves).ToList();
        var vertexPixel = resolved.Count(pipeline => pipeline.Stage(PipelineSlot.Vertex).HasValue && pipeline.Stage(PipelineSlot.Pixel).HasValue);
        var meshPixel = resolved.Count(pipeline => pipeline.Stage(PipelineSlot.Mesh).HasValue && pipeline.Stage(PipelineSlot.Pixel).HasValue);
        var compute = cache.Compute.Count(pipeline => map.Programs(pipeline.Shader).Count > 0);
        Console.WriteLine($"graphics resolved {resolved.Count} of {cache.Graphics.Count} (vs+ps {vertexPixel}, ms+ps {meshPixel}); compute resolved {compute} of {cache.Compute.Count}");
    }

    private static void Export(PipelineCache cache, CacheProgramMap programs, string output)
    {
        var code = Path.Combine(output, CodeFolder);
        Directory.CreateDirectory(code);
        var written = 0;

        for (var shader = 0; shader < cache.Header.ShaderCount; shader++)
        {
            var bytes = cache.Shader(shader);

            if (!bytes.HasValue)
            {
                continue;
            }

            var name = shader.ToString("d5", CultureInfo.InvariantCulture) + ".bin";
            File.WriteAllBytes(Path.Combine(code, name), bytes.Value.ToArray());
            written++;
        }

        PipelineCacheJson.Write(Path.Combine(output, RecordsFile), cache, programs);
        Console.WriteLine($"wrote {written} shaders to {code} and {Path.Combine(output, RecordsFile)}");
    }

    private static string Top<T>(IEnumerable<T> values)
    {
        var groups = values.GroupBy(value => value).OrderByDescending(group => group.Count()).Take(TopValues);

        return string.Join(", ", groups.Select(group => $"{group.Key}: {group.Count()}"));
    }
}
