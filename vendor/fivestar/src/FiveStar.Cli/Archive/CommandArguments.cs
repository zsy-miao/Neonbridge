// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;

namespace FiveStar.Cli.Archive;

/// <summary>
/// The arguments of one command: positional values, <c>--name value</c> options and bare <c>--flag</c> switches.
/// Every option takes one value unless it is listed as a flag.
/// </summary>
internal sealed class CommandArguments
{
    private const string OptionPrefix = "--";

    private readonly List<string> _positionals = [];
    private readonly Dictionary<string, string> _options = new(StringComparer.Ordinal);
    private readonly HashSet<string> _flags = new(StringComparer.Ordinal);

    public CommandArguments(IReadOnlyList<string> args, IReadOnlyCollection<string> flagNames)
    {
        for (var index = 0; index < args.Count; index++)
        {
            var arg = args[index];

            if (!arg.StartsWith(OptionPrefix, StringComparison.Ordinal))
            {
                _positionals.Add(arg);

                continue;
            }

            if (flagNames.Contains(arg))
            {
                _flags.Add(arg);

                continue;
            }

            if (index + 1 < args.Count)
            {
                _options[arg] = args[index + 1];
                index++;
            }
        }
    }

    public IReadOnlyList<string> Positionals => _positionals;

    public bool HasFlag(string name)
    {
        return _flags.Contains(name);
    }

    public string? Option(string name)
    {
        _options.TryGetValue(name, out var value);

        return value;
    }

    public long LongOption(string name, long fallback)
    {
        var value = Option(name);

        if (value is null)
        {
            return fallback;
        }

        return long.Parse(value, CultureInfo.InvariantCulture);
    }
}
