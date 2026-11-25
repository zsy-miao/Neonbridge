// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Cli.Archive;
using FiveStar.Cli.Exe;

namespace FiveStar.Cli;

/// <summary>The <c>fivestar exe</c> commands: read the game executable and report what the port needs.</summary>
internal static class ExeCommands
{
    private const int Success = 0;
    private const int UsageError = 2;

    private static readonly string[] Flags = [];

    public static int Run(IReadOnlyList<string> args)
    {
        if (args.Count == 0)
        {
            PrintHelp(Console.Error);

            return UsageError;
        }

        if (args[0] == "help")
        {
            PrintHelp(Console.Out);

            return Success;
        }

        if (args[0] != "analyze")
        {
            PrintHelp(Console.Error);

            return UsageError;
        }

        var arguments = new CommandArguments(args.Skip(1).ToList(), Flags);

        if (arguments.Positionals.Count < 1)
        {
            PrintHelp(Console.Error);

            return UsageError;
        }

        return AnalyzeCommand.Run(arguments);
    }

    private static void PrintHelp(TextWriter output)
    {
        output.WriteLine("fivestar exe <command>");
        output.WriteLine("  analyze <exe> [--dump <folder>] [--gdk-lib <xgameruntime.lib>] [--out <dir>] [--loader <fivestar-loader>]");
        output.WriteLine("           read the executable and write a Markdown + JSON report: PE basics, imports vs the loader,");
        output.WriteLine("           GDK call sites, D3D12.X usage, archive and shader references, and a day-one gap list.");
        output.WriteLine("           --dump and --gdk-lib default to FIVESTAR_DUMP and FIVESTAR_GDK_LIB.");
    }
}
