// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Cli.Archive;
using FiveStar.Cli.Model;

namespace FiveStar.Cli;

/// <summary>The <c>fivestar model</c> commands: drawables (meshlet geometry) carved from an archive.</summary>
internal static class ModelCommands
{
    private const int Success = 0;
    private const int UsageError = 2;
    private const int TwoArguments = 2;

    private static readonly string[] Flags = ["--list"];

    public static int Run(IReadOnlyList<string> args)
    {
        if (args.Count == 0)
        {
            PrintHelp(Console.Error);

            return UsageError;
        }

        var arguments = new CommandArguments(args.Skip(1).ToList(), Flags);
        using var output = ConsoleOutput.Open();
        var reports = new ModelReports(output);

        switch (args[0])
        {
            case "scan":
                return RunScan(reports, arguments);
            case "export":
                return RunExport(reports, arguments);
            case "help":
                PrintHelp(output);

                return Success;
            default:
                PrintHelp(Console.Error);

                return UsageError;
        }
    }

    private static int RunScan(ModelReports reports, CommandArguments arguments)
    {
        if (arguments.Positionals.Count < 1)
        {
            return Usage("model scan <archive.rpf> [--from 0xOFFSET] [--limit-mb N] [--list]");
        }

        reports.Scan(arguments.Positionals[0], arguments);

        return Success;
    }

    private static int RunExport(ModelReports reports, CommandArguments arguments)
    {
        if (arguments.Positionals.Count < TwoArguments)
        {
            return Usage("model export <archive.rpf> <out dir> [--from 0xOFFSET] [--limit-mb N] [--name TEXT] [--lod N]");
        }

        reports.Export(arguments.Positionals[0], arguments.Positionals[1], arguments);

        return Success;
    }

    private static int Usage(string usage)
    {
        Console.Error.WriteLine("usage: fivestar " + usage);

        return UsageError;
    }

    private static void PrintHelp(TextWriter output)
    {
        output.WriteLine("fivestar model <command>");
        output.WriteLine("  scan   <archive.rpf> [--from 0xOFFSET] [--limit-mb N] [--list]");
        output.WriteLine("         drawables and geometries that decode, per level of detail, and why the rest do not");
        output.WriteLine("  export <archive.rpf> <out dir> [--from 0xOFFSET] [--limit-mb N] [--name TEXT] [--lod N]");
        output.WriteLine("         one Wavefront OBJ per drawable (level of detail 0 unless --lod)");
    }
}
