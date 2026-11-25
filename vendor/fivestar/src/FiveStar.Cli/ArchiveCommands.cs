// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Cli.Archive;

namespace FiveStar.Cli;

/// <summary>The <c>fivestar archive</c> commands: RPF9 headers, entry carving, Bink videos and PSO files.</summary>
internal static class ArchiveCommands
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

        var rest = args.Skip(1).ToList();

        if (args[0] == "pso")
        {
            return PsoCommands.Run(rest);
        }

        var arguments = new CommandArguments(rest, Flags);
        using var output = ConsoleOutput.Open();
        var reports = new ArchiveReports(output);

        switch (args[0])
        {
            case "header":
                return RunHeader(reports, arguments);
            case "words":
                return RunWords(reports, arguments);
            case "stats":
                return RunStats(reports, arguments);
            case "carve":
                return RunCarve(reports, arguments);
            case "bink":
                return RunBink(reports, arguments);
            case "help":
                return RunHelp(output);
            default:
                return RunUnknown();
        }
    }

    private static int RunHeader(ArchiveReports reports, CommandArguments arguments)
    {
        if (arguments.Positionals.Count < 1)
        {
            return Usage("archive header <dump dir>");
        }

        reports.Headers(arguments.Positionals[0]);

        return Success;
    }

    private static int RunWords(ArchiveReports reports, CommandArguments arguments)
    {
        if (arguments.Positionals.Count < 1)
        {
            return Usage("archive words <dump dir>");
        }

        reports.Words(arguments.Positionals[0]);

        return Success;
    }

    private static int RunStats(ArchiveReports reports, CommandArguments arguments)
    {
        if (arguments.Positionals.Count < 1)
        {
            return Usage("archive stats <archive.rpf> [--samples N]");
        }

        reports.Stats(arguments.Positionals[0], arguments);

        return Success;
    }

    private static int RunCarve(ArchiveReports reports, CommandArguments arguments)
    {
        if (arguments.Positionals.Count < TwoArguments)
        {
            return Usage("archive carve <archive.rpf> <out dir> [--limit-mb N]");
        }

        reports.Carve(arguments.Positionals[0], arguments.Positionals[1], arguments);

        return Success;
    }

    private static int RunBink(ArchiveReports reports, CommandArguments arguments)
    {
        if (arguments.Positionals.Count < 1)
        {
            return Usage("archive bink <archive.rpf> [<out dir>] [--list]");
        }

        var outputDirectory = arguments.Positionals.Count > 1 ? arguments.Positionals[1] : ".";
        reports.Bink(arguments.Positionals[0], outputDirectory, arguments);

        return Success;
    }

    private static int RunHelp(TextWriter output)
    {
        PrintHelp(output);

        return Success;
    }

    private static int RunUnknown()
    {
        PrintHelp(Console.Error);

        return UsageError;
    }

    private static int Usage(string usage)
    {
        Console.Error.WriteLine("usage: fivestar " + usage);

        return UsageError;
    }

    private static void PrintHelp(TextWriter output)
    {
        output.WriteLine("fivestar archive <command>");
        output.WriteLine("  header <dump dir>                         header fields of every .rpf, smallest first");
        output.WriteLine("  words  <dump dir>                         raw header words and entropy of every .rpf");

        output.WriteLine("  stats  <archive.rpf> [--samples N]        share of sampled bytes in plain zlib streams");
        output.WriteLine("  carve  <archive.rpf> <out> [--limit-mb N] write every recoverable entry as <offset>.<kind>.bin");
        output.WriteLine("  bink   <archive.rpf> [<out>] [--list]     find (and write) Bink 2 videos stored raw");
        output.WriteLine("  pso    <command> ...                      PSO files and PRD0 resources (fivestar archive pso help)");
    }
}
