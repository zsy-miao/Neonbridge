// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Cli.Archive;
using FiveStar.Cli.Audio;

namespace FiveStar.Cli;

/// <summary>The <c>fivestar audio</c> commands over AWC containers stored raw in an archive.</summary>
internal static class AudioCommands
{
    private const int Success = 0;
    private const int UsageError = 2;
    private const int TwoArguments = 2;

    public static int Run(IReadOnlyList<string> args)
    {
        if (args.Count == 0)
        {
            PrintHelp(Console.Error);

            return UsageError;
        }

        var arguments = new CommandArguments(args.Skip(1).ToList(), [AudioReports.DecodeXmaFlag]);
        using var output = ConsoleOutput.Open();
        var reports = new AudioReports(output);

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

    private static int RunScan(AudioReports reports, CommandArguments arguments)
    {
        if (arguments.Positionals.Count < 1)
        {
            return Usage("audio scan <archive.rpf> [--limit N]");
        }

        reports.Scan(arguments.Positionals[0], arguments);

        return Success;
    }

    private static int RunExport(AudioReports reports, CommandArguments arguments)
    {
        if (arguments.Positionals.Count < TwoArguments)
        {
            return Usage("audio export <archive.rpf> <out dir> [--limit N] [--codec opus|xma2|pcm16|pcmfloat] [--from hex offset] [--decode-xma]");
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
        output.WriteLine("fivestar audio <command>");
        output.WriteLine("  scan   <archive.rpf> [--limit N]          AWC containers and tracks per codec (first N containers)");
        output.WriteLine("  export <archive.rpf> <out> [--limit N] [--codec C] [--from X] [--decode-xma]");
        output.WriteLine("                                            write tracks (first N, of codec C, from offset X) as .opus, .wav or .raw;");
        output.WriteLine("                                            --decode-xma writes XMA2 as decoded 16-bit PCM instead of XMA2 in WAVE");
    }
}
