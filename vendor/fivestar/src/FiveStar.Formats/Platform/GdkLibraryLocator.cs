// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Formats.Platform;

/// <summary>Finds the 64-bit <c>xgameruntime.lib</c> of an installed PC GDK.</summary>
public static class GdkLibraryLocator
{
    public const string LibraryName = "xgameruntime.lib";

    private const string GdkFolder = "Microsoft GDK";

    // The 64-bit library sits in an amd64 or x64 folder of the GDK.
    private static readonly string[] SixtyFourBitFolders = ["amd64", "x64"];

    /// <summary>The newest 64-bit library under <c>&lt;programFilesX86&gt;\Microsoft GDK</c>, or null.</summary>
    public static string? FindInstalled(string programFilesX86)
    {
        var root = Path.Combine(programFilesX86, GdkFolder);

        if (!Directory.Exists(root))
        {
            return null;
        }

        return Directory.EnumerateFiles(root, LibraryName, SearchOption.AllDirectories)
            .Where(path => SixtyFourBitFolders.Contains(Path.GetFileName(Path.GetDirectoryName(path)), StringComparer.OrdinalIgnoreCase))
            .Order(StringComparer.Ordinal)
            .LastOrDefault();
    }
}
