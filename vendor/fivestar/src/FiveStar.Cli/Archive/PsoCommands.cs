// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Cli.Archive;

/// <summary>The <c>fivestar archive pso</c> commands over carved PSO files and PRD0 resources.</summary>
internal static class PsoCommands
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

        var arguments = new CommandArguments(args.Skip(1).ToList(), Flags);
        using var output = ConsoleOutput.Open();
        var reports = new PsoReports(output);

        if (args[0] == "help")
        {
            PrintHelp(output);

            return Success;
        }

        var needed = args[0] == "crack" ? TwoArguments : 1;

        if (arguments.Positionals.Count < needed)
        {
            PrintHelp(Console.Error);

            return UsageError;
        }

        return Dispatch(args[0], reports, arguments);
    }

    private static int Dispatch(string command, PsoReports reports, CommandArguments arguments)
    {
        switch (command)
        {
            case "info":
                return reports.Info(arguments.Positionals[0]);
            case "schema":
                return reports.Schema(arguments.Positionals[0]);
            case "dump":
                return reports.Dump(arguments.Positionals[0], arguments);
            case "check":
                return reports.Check(arguments.Positionals);
            case "roots":
                return reports.Roots(arguments.Positionals, arguments);
            case "hashes":
                return reports.Hashes(arguments.Positionals, arguments);
            case "crack":
                return reports.Crack(arguments.Positionals[0], arguments.Positionals[1], arguments);
            default:
                PrintHelp(Console.Error);

                return UsageError;
        }
    }

    private static void PrintHelp(TextWriter output)
    {
        output.WriteLine("fivestar archive pso <command>   (<blob> = a carved .psin.bin or .prd0.bin)");
        output.WriteLine("  info   <blob>                          chunks, blocks, root type");
        output.WriteLine("  schema <blob>                          struct, member and enum tables");

        output.WriteLine("  dump   <blob> [--max N] [--chars N]    the root object as JSON (names are hashes)");
        output.WriteLine("  check  <dir|blob> ...                  walk every PSO file and PRD0 resource, count errors");
        output.WriteLine("  roots  <dir> ... [--top N]             census of root struct hashes (file / resource type)");
        output.WriteLine("  hashes <dir> ... [--out F]             harvest struct, member, enum, value and data hashes");
        output.WriteLine("  crack  <hashes.json> <words.txt> [--out F]   joaat a word list against a harvest");
    }
}
