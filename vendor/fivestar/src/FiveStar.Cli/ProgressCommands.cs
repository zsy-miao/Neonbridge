// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Cli.Archive;
using FiveStar.Cli.Progress;

namespace FiveStar.Cli;

/// <summary>The <c>fivestar progress</c> commands: collect the project status and render it as an SVG.</summary>
internal static class ProgressCommands
{
    private const int Success = 0;
    private const int UsageError = 2;
    private const int TwoArguments = 2;

    private static readonly string[] Flags = [];

    public static int Run(IReadOnlyList<string> args)
    {
        if (args.Count == 0)
        {
            PrintHelp(Console.Error);

            return UsageError;
        }

        switch (args[0])
        {
            case "collect":
                return RunCollect(new CommandArguments(args.Skip(1).ToList(), Flags));
            case "help":
                return RunHelp();
            default:
                return RunRender(new CommandArguments(args, Flags));
        }
    }

    private static int RunCollect(CommandArguments arguments)
    {
        if (arguments.Positionals.Count < 1)
        {
            return Usage("progress collect [--shaders <results.json>] [--dump <dir>] [--manual <manual.json>] <status.json>");
        }

        var statusPath = arguments.Positionals[0];
        ProgressStatus? previous = null;

        if (File.Exists(statusPath))
        {
            previous = StatusFile.Read(statusPath);
        }

        var collector = new StatusCollector(Console.Error);
        var status = collector.Collect(arguments.Option("--shaders"), arguments.Option("--dump"), arguments.Option("--manual"), previous);
        StatusFile.Write(statusPath, status);
        Console.WriteLine($"wrote {statusPath}: {status.Areas.Count} areas");

        return Success;
    }

    private static int RunRender(CommandArguments arguments)
    {
        if (arguments.Positionals.Count < TwoArguments)
        {
            return Usage("progress <status.json> <out.svg>");
        }

        var status = StatusFile.Read(arguments.Positionals[0]);
        var output = arguments.Positionals[1];
        File.WriteAllText(output, ProgressSvg.Render(status));
        Console.WriteLine("wrote " + output);

        return Success;
    }

    private static int RunHelp()
    {
        PrintHelp(Console.Out);

        return Success;
    }

    private static int Usage(string usage)
    {
        Console.Error.WriteLine("usage: fivestar " + usage);

        return UsageError;
    }

    private static void PrintHelp(TextWriter output)
    {
        output.WriteLine("fivestar progress <command>");
        output.WriteLine("  <status.json> <out.svg>            render the progress report as an SVG (light and dark theme)");
        output.WriteLine("  collect [--shaders <results.json>] [--dump <dir>] [--manual <manual.json>] <status.json>");
        output.WriteLine("                                     build status.json: shader stages from a translate run, textures");
        output.WriteLine("                                     and archive bytes from a dump, the rest from the manual file;");
        output.WriteLine("                                     areas without a new source keep their previous value");
    }
}
