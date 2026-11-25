// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.IO.Enumeration;

namespace FiveStar.Shaders.Cache;

/// <summary>
/// The <c>.fssm</c> files of a program folder in ordinal order, with a stamp over every name, length and write time.
/// Listing a folder reads directory entries only; no program file is opened.
/// </summary>
public sealed record ProgramFolderListing(IReadOnlyList<string> Paths, ulong Stamp)
{
    private const string ProgramPattern = "*.fssm";

    // FNV-1a, 64-bit (http://www.isthe.com/chongo/tech/comp/fnv/).
    private const ulong FnvOffsetBasis = 0xCBF29CE484222325;
    private const ulong FnvPrime = 0x100000001B3;
    private const int ByteBits = 8;

    /// <summary>Lists <paramref name="folder"/>.</summary>
    public static ProgramFolderListing Of(string folder)
    {
        var options = new EnumerationOptions { RecurseSubdirectories = false, IgnoreInaccessible = true };
        var files = new FileSystemEnumerable<(string Path, long Length, long WriteTicks)>(folder, Describe, options)
        {
            ShouldIncludePredicate = IsProgram,
        };
        var sorted = files.OrderBy(file => file.Path, StringComparer.Ordinal).ToList();
        ulong stamp = FnvOffsetBasis;

        foreach (var file in sorted)
        {
            stamp = Mix(stamp, file.Path);
            stamp = Mix(stamp, (ulong)file.Length);
            stamp = Mix(stamp, (ulong)file.WriteTicks);
        }

        return new ProgramFolderListing([.. sorted.Select(file => file.Path)], stamp);
    }

    private static (string Path, long Length, long WriteTicks) Describe(ref FileSystemEntry entry)
    {
        return (entry.ToFullPath(), entry.Length, entry.LastWriteTimeUtc.UtcTicks);
    }

    private static bool IsProgram(ref FileSystemEntry entry)
    {
        return !entry.IsDirectory && FileSystemName.MatchesSimpleExpression(ProgramPattern, entry.FileName);
    }

    private static ulong Mix(ulong hash, string text)
    {
        foreach (char c in text)
        {
            hash = Mix(hash, c);
        }

        return hash;
    }

    private static ulong Mix(ulong hash, ulong value)
    {
        for (int shift = 0; shift < sizeof(ulong) * ByteBits; shift += ByteBits)
        {
            hash = (hash ^ (byte)(value >> shift)) * FnvPrime;
        }

        return hash;
    }
}
