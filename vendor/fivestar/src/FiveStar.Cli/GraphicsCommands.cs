// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Cli.Graphics;

namespace FiveStar.Cli;

/// <summary>The <c>fivestar graphics</c> commands: rendering with translated programs on our own D3D12 code.</summary>
internal static class GraphicsCommands
{
    private const int Success = 0;
    private const int UsageError = 2;

    private static readonly string[] Usage =
    [
        "usage: fivestar graphics <command> [arguments]",
        "  " + DrawSampleCommand.Usage,
        "        draw a game texture with the translated immediate-mode unlit pass and the pipeline cache's state",
        "  " + DrawModelCommand.Usage,
        "        draw a drawable carved from an archive with the same pass, its diffuse texture and its normals",
    ];

    public static int Run(IReadOnlyList<string> args)
    {
        if (args.Count == 0)
        {
            PrintUsage();

            return UsageError;
        }

        try
        {
            switch (args[0])
            {
                case "draw-sample":
                    return DrawSampleCommand.Run(args.Skip(1).ToList());
                case "draw-model":
                    return DrawModelCommand.Run(args.Skip(1).ToList());
                case "help":
                    PrintUsage();

                    return Success;
                default:
                    PrintUsage();

                    return UsageError;
            }
        }
        catch (ArgumentException exception)
        {
            Console.Error.WriteLine($"fivestar graphics: {exception.Message}");

            return UsageError;
        }
    }

    private static void PrintUsage()
    {
        foreach (var line in Usage)
        {
            Console.Error.WriteLine(line);
        }
    }
}
