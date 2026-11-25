// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Cli.Archive;
using FiveStar.Cli.Platform;

namespace FiveStar.Cli;

/// <summary>The <c>fivestar platform</c> commands: shipped modules, the OS volume header and the package config hive.</summary>
internal static class PlatformCommands
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

        var arguments = new CommandArguments(args.Skip(1).ToList(), Flags);
        using var output = ConsoleOutput.Open();
        var reports = new PlatformReports(output);

        if (args[0] == "help")
        {
            PrintHelp(output);

            return Success;
        }

        if (arguments.Positionals.Count < 1)
        {
            PrintHelp(Console.Error);

            return UsageError;
        }

        if (args[0] == "gdk-abi")
        {
            return GdkAbiCommand.Run(arguments);
        }

        return Dispatch(args[0], reports, arguments.Positionals);
    }

    private static int Dispatch(string command, PlatformReports reports, IReadOnlyList<string> positionals)
    {
        switch (command)
        {
            case "imports":
                reports.Imports(positionals[0]);

                return Success;
            case "pe":
                reports.Summaries(positionals);

                return Success;
            case "xvd":
                reports.Xvd(positionals[0]);

                return Success;
            case "regf":
                reports.Registry(positionals[0]);

                return Success;
            default:
                PrintHelp(Console.Error);

                return UsageError;
        }
    }

    private static void PrintHelp(TextWriter output)
    {
        output.WriteLine("fivestar platform <command>");
        output.WriteLine("  imports <dir>          which functions of which DLLs the shipped modules import (system DLLs last)");
        output.WriteLine("  pe      <module> ...   headers, sections, imports, exports, version, PDB path and resources");
        output.WriteLine("  xvd     <file.xvd>     the plain XVD volume header");
        output.WriteLine("  regf    <hive>         every key and value of a registry hive (microsoft_gameconfig.bin)");

        output.WriteLine("  gdk-abi <xgameruntime.lib> [module ...] [--out file]");
        output.WriteLine("                         the loader's --gdk-abi table from the PC GDK library; per module, how many");
        output.WriteLine("                         of its GDK calls the table names");
    }
}
