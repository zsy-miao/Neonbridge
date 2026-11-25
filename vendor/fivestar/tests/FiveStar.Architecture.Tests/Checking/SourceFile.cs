// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Architecture.Tests.Checking;

/// <summary>A file of the repository: its path relative to the root with <c>/</c> separators, and its text unless it is binary.</summary>
public sealed record SourceFile(string Path, string? Text)
{
    public string Extension => System.IO.Path.GetExtension(Path).ToLowerInvariant();

    public string Name => System.IO.Path.GetFileNameWithoutExtension(Path);

    /// <summary>The lines of the text without their line ends; empty for a binary file.</summary>
    public IReadOnlyList<string> Lines()
    {
        if (Text is null)
        {
            return [];
        }

        return Text.Split('\n').Select(line => line.TrimEnd('\r')).ToArray();
    }
}
