// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Text;

namespace FiveStar.Cli.Archive;

/// <summary>Opens standard output as a buffered UTF-8 writer for reports that print many lines.</summary>
internal static class ConsoleOutput
{
    private const int BufferSize = 1 << 16;

    public static StreamWriter Open()
    {
        var writer = new StreamWriter(Console.OpenStandardOutput(), new UTF8Encoding(false), BufferSize);

        return writer;
    }
}
