// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Diagnostics;
using FiveStar.Cli.Shader;
using FiveStar.Formats.Images;
using FiveStar.Graphics;
using FiveStar.Shaders.Disassembly;

namespace FiveStar.Cli.Graphics;

/// <summary>
/// <c>fivestar graphics draw-sample</c>: draws a real texture with the game's translated immediate-mode unlit pass and
/// the pipeline cache's state, offscreen, and writes the image as PNG.
/// </summary>
internal static class DrawSampleCommand
{
    public const string Usage =
        "draw-sample <out.png> [--dump <folder>] [--programs <fssm folder>] [--texture <file.ttpf>] [--index N] [--adapter <index|name|warp>] [--work <dir>] [--debug-layer]";

    private const int Success = 0;
    private const int Failure = 1;
    private const string DumpVariable = "FIVESTAR_DUMP";
    private const string ProgramsVariable = "FIVESTAR_SHADER_DIR";
    private const string DebugFlag = "debug-layer";
    private const string Warp = "warp";
    private const string DefaultTexture = "data/ttdb/ui/platform.ttpf";

    public static int Run(IReadOnlyList<string> args)
    {
        var arguments = new CommandArguments(args, new HashSet<string> { DebugFlag });

        if (arguments.Positional.Count != 1)
        {
            throw new ArgumentException(Usage);
        }

        var output = Path.GetFullPath(arguments.Positional[0]);
        var dump = Required(arguments.Option("dump") ?? Environment.GetEnvironmentVariable(DumpVariable), "--dump or " + DumpVariable);
        var programs = Required(arguments.Option("programs") ?? Environment.GetEnvironmentVariable(ProgramsVariable), "--programs or " + ProgramsVariable);
        var texture = arguments.Option("texture") ?? Path.Combine(dump, DefaultTexture);
        var work = arguments.Option("work") ?? Path.Combine(Path.GetDirectoryName(output)!, Path.GetFileNameWithoutExtension(output) + "-shaders");
        var clock = Stopwatch.StartNew();

        var inputs = SampleInputs.Load(dump, programs, texture, arguments.IntegerOption("index") ?? 0, work, new RdnaDecoder(), ShaderTools.Dxc(arguments));
        Console.WriteLine($"pipeline {inputs.Pipeline.Record.Index}: {inputs.Pipeline.VertexProgram} + {inputs.Pipeline.PixelProgram}");
        Console.WriteLine($"texture {texture} #{arguments.IntegerOption("index") ?? 0}: {inputs.Texture.Width}x{inputs.Texture.Height} {inputs.Texture.Format}");

        using var device = Open(arguments.Option("adapter"), arguments.Flag(DebugFlag));
        var frame = UnlitRenderer.Render(device, inputs.Pipeline, inputs.Vertex, inputs.Pixel, inputs.Texture);
        PngWriter.Write(output, frame.Scene.Width, frame.Scene.Height, frame.Pixels);
        Console.WriteLine($"{device.AdapterName}: wrote {output} ({frame.Scene.Width}x{frame.Scene.Height}) in {clock.Elapsed.TotalSeconds:F1} s");

        foreach (var error in frame.Errors)
        {
            Console.Error.WriteLine($"debug layer: {error}");
        }

        if (frame.Errors.Count > 0)
        {
            return Failure;
        }

        return Success;
    }

    private static GpuDevice Open(string? adapter, bool debugLayer)
    {
        if (string.Equals(adapter, Warp, StringComparison.OrdinalIgnoreCase))
        {
            return GpuDevice.CreateWarp(debugLayer);
        }

        return new GpuDevice(PsoCommand.SelectAdapter(adapter).Index, debugLayer);
    }

    private static string Required(string? value, string source)
    {
        if (string.IsNullOrWhiteSpace(value))
        {
            throw new ArgumentException($"set {source}");
        }

        return value;
    }
}
