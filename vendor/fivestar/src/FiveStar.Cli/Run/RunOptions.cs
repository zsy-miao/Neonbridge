// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Cli.Shader;

namespace FiveStar.Cli.Run;

/// <summary>
/// The command line of <c>fivestar run</c> and <c>fivestar doctor</c>: the dump folder, the options of this command, the
/// tool options passed on to the setup steps (<see cref="Tools"/>) and everything after <c>--</c> for the loader.
/// </summary>
internal sealed record RunOptions(
    string Dump,
    string? Work,
    bool Debug,
    int? Timeout,
    string? Programs,
    string? Translated,
    string? Loader,
    CommandArguments Tools,
    IReadOnlyList<string> LoaderArguments)
{
    /// <summary>Separates the options of this command from the extra loader options.</summary>
    public const string Separator = "--";

    public const string DebugFlag = "debug";

    private const string OptionPrefix = "--";

    private static readonly HashSet<string> Flags = [DebugFlag, Setup.SetupCommand.TranslateAheadFlag];

    private static readonly HashSet<string> ValueOptions = ["work", "timeout", "shaders", "translated", "loader", "dxc", "gdk-lib", "jobs", "bcpack-tables"];

    /// <summary>Whether prebuilt shader folders replace the shader steps of setup.</summary>
    public bool PrebuiltShaders => Programs is not null;

    /// <summary>Whether every program is translated before the boot, not by the loader on first use.</summary>
    public bool TranslateAhead => Tools.Flag(Setup.SetupCommand.TranslateAheadFlag);

    /// <summary>Reads the arguments; throws <see cref="ArgumentException"/> with the reason when they are wrong.</summary>
    public static RunOptions Parse(IReadOnlyList<string> args)
    {
        var split = args.ToList().IndexOf(Separator);
        var own = args.ToList();
        var extra = new List<string>();

        if (split >= 0)
        {
            own = args.Take(split).ToList();
            extra = args.Skip(split + 1).ToList();
        }

        var unknown = own.FirstOrDefault(argument => argument.StartsWith(OptionPrefix, StringComparison.Ordinal) && !Known(argument[OptionPrefix.Length..]));

        if (unknown is not null)
        {
            throw new ArgumentException($"unknown option {unknown}; loader options go after {Separator}");
        }

        var arguments = new CommandArguments(own, Flags);

        if (arguments.Positional.Count != 1)
        {
            throw new ArgumentException("give one dump folder");
        }

        return Create(arguments, extra);
    }

    private static RunOptions Create(CommandArguments arguments, IReadOnlyList<string> extra)
    {
        var timeout = arguments.IntegerOption("timeout");
        var programs = Full(arguments.Option("shaders"));
        var translated = Full(arguments.Option("translated"));

        if (timeout is not null && timeout < 1)
        {
            throw new ArgumentException("--timeout takes a number of seconds from 1");
        }

        if ((programs is null) != (translated is null))
        {
            throw new ArgumentException("--shaders and --translated go together");
        }

        return new RunOptions(
            Path.GetFullPath(arguments.Positional[0]),
            Full(arguments.Option("work")),
            arguments.Flag(DebugFlag),
            timeout,
            programs,
            translated,
            Full(arguments.Option("loader")),
            arguments,
            extra);
    }

    private static string? Full(string? path)
    {
        if (path is null)
        {
            return null;
        }

        return Path.GetFullPath(path);
    }

    private static bool Known(string name) => Flags.Contains(name) || ValueOptions.Contains(name);
}
