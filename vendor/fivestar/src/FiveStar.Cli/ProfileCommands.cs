// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Cli.Archive;
using FiveStar.Cli.Profile;

namespace FiveStar.Cli;

/// <summary>The <c>fivestar profile</c> commands: read the loader's managed CPU profiles.</summary>
internal static class ProfileCommands
{
    private const int Success = 0;
    private const int UsageError = 2;
    private const int DefaultTop = 30;

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
            case "summary":
                return RunSummary(new CommandArguments(args.Skip(1).ToList(), Flags));
            case "help":
                PrintHelp(Console.Out);

                return Success;
            default:
                PrintHelp(Console.Error);

                return UsageError;
        }
    }

    private static int RunSummary(CommandArguments arguments)
    {
        if (arguments.Positionals.Count != 1 || !File.Exists(arguments.Positionals[0]))
        {
            Console.Error.WriteLine("usage: fivestar profile summary <trace.nettrace> [--top <n>]");

            return UsageError;
        }

        var summary = ManagedProfileSummary.Read(arguments.Positionals[0]);
        summary.Write(Console.Out, (int)arguments.LongOption("--top", DefaultTop));

        return Success;
    }

    private static void PrintHelp(TextWriter output)
    {
        output.WriteLine("fivestar profile <command>");
        output.WriteLine("  summary <trace.nettrace> [--top <n>]  a loader managed profile (--profile-managed) by thread class (game,");
        output.WriteLine("                                        build, queue, loader, runtime, other): samples in and outside managed");
        output.WriteLine("                                        code, the top methods by exclusive and inclusive samples (30)");
    }
}
