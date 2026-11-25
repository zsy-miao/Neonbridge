// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;

namespace FiveStar.Cli.Shader;

/// <summary>The arguments of one command: positional values, <c>--name value</c> options and <c>--flag</c> switches.</summary>
internal sealed class CommandArguments
{
    private const string OptionPrefix = "--";

    private readonly List<string> _positional = [];
    private readonly Dictionary<string, string> _options = [];
    private readonly HashSet<string> _flags = [];

    /// <summary>Splits the arguments; names listed in <paramref name="flagNames"/> take no value.</summary>
    public CommandArguments(IReadOnlyList<string> arguments, IReadOnlySet<string> flagNames)
    {
        for (var index = 0; index < arguments.Count; index++)
        {
            var argument = arguments[index];

            if (!argument.StartsWith(OptionPrefix, StringComparison.Ordinal))
            {
                _positional.Add(argument);
                continue;
            }

            var name = argument[OptionPrefix.Length..];

            if (flagNames.Contains(name))
            {
                _flags.Add(name);
                continue;
            }

            if (index + 1 >= arguments.Count)
            {
                throw new ArgumentException($"--{name} needs a value");
            }

            index++;
            _options[name] = arguments[index];
        }
    }

    public IReadOnlyList<string> Positional => _positional;

    public string? Option(string name)
    {
        return _options.GetValueOrDefault(name);
    }

    public int? IntegerOption(string name)
    {
        var value = Option(name);

        if (value is null)
        {
            return null;
        }

        return int.Parse(value, CultureInfo.InvariantCulture);
    }

    public bool Flag(string name)
    {
        return _flags.Contains(name);
    }
}
