// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Text;

namespace FiveStar.Debugger.Session;

/// <summary>Builds a CreateProcess command line that CommandLineToArgvW and the C runtime split back into the same arguments.</summary>
internal static class CommandLine
{
    /// <summary>Backslashes before a quote are doubled; one more escapes the quote itself.</summary>
    private const int EscapedBackslashes = 2;

    private static readonly char[] NeedsQuotes = [' ', '\t', '\n', '\v', '"'];

    public static string Build(string program, IReadOnlyList<string> arguments)
    {
        var line = new StringBuilder();
        Append(line, program);

        foreach (string argument in arguments)
        {
            line.Append(' ');
            Append(line, argument);
        }

        return line.ToString();
    }

    private static void Append(StringBuilder line, string argument)
    {
        if (argument.Length > 0 && argument.IndexOfAny(NeedsQuotes) < 0)
        {
            line.Append(argument);

            return;
        }

        line.Append('"');
        int backslashes = 0;

        foreach (char c in argument)
        {
            if (c == '\\')
            {
                backslashes++;

                continue;
            }

            int escapes = backslashes;

            if (c == '"')
            {
                escapes = (backslashes * EscapedBackslashes) + 1;
            }

            line.Append('\\', escapes).Append(c);
            backslashes = 0;
        }

        line.Append('\\', backslashes * EscapedBackslashes).Append('"');
    }
}
