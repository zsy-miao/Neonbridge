// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Text;
using FiveStar.Formats.Platform;

namespace FiveStar.Cli.Exe;

/// <summary>
/// Finds, with addresses, where the image refers to known constants and strings: the <c>RPF9</c> magic (0x39465052)
/// and the archive names, as a pointer into the archive-parsing code, and the <c>shaders.rpf</c> reference for shader
/// linkage. It reads bytes only; it recovers no keys and decrypts nothing.
/// </summary>
internal static class ReferenceScan
{
    private const string CodeSectionPrefix = ".text";
    private const string ArchiveMagic = "RPF9";
    private const string ShaderArchive = "shaders.rpf";
    private const int MaxHitsPerPattern = 64;

    public static IReadOnlyList<SymbolReference> ArchiveReferences(PeImage image, IReadOnlyList<string> archiveNames)
    {
        var references = new List<SymbolReference>();
        references.AddRange(Occurrences(image, Encoding.ASCII.GetBytes(ArchiveMagic), "RPF9 (0x39465052)"));

        foreach (var name in archiveNames)
        {
            references.AddRange(Occurrences(image, Encoding.ASCII.GetBytes(name), name));
        }

        return references;
    }

    public static IReadOnlyList<SymbolReference> ShaderReferences(PeImage image)
    {
        return Occurrences(image, Encoding.ASCII.GetBytes(ShaderArchive), ShaderArchive);
    }

    private static List<SymbolReference> Occurrences(PeImage image, byte[] pattern, string what)
    {
        var results = new List<SymbolReference>();

        foreach (var section in image.Sections)
        {
            ScanSection(image, section, pattern, what, results);
        }

        return results;
    }

    private static void ScanSection(PeImage image, PeSection section, byte[] pattern, string what, List<SymbolReference> results)
    {
        var start = (int)image.RawStart(section);
        var length = (int)Math.Min(section.RawSize, image.Data.Length - start);

        if (length <= 0)
        {
            return;
        }

        var span = image.Data.AsSpan(start, length);
        var position = 0;

        while (position < span.Length)
        {
            var hit = span[position..].IndexOf(pattern);

            if (hit < 0 || results.Count >= MaxHitsPerPattern)
            {
                break;
            }

            var at = position + hit;
            results.Add(new SymbolReference(what, (uint)(section.VirtualAddress + at), section.Name, Note(section.Name)));
            position = at + 1;
        }
    }

    private static string Note(string sectionName)
    {
        if (sectionName.StartsWith(CodeSectionPrefix, StringComparison.Ordinal))
        {
            return "immediate in code";
        }

        return "constant or string";
    }
}
