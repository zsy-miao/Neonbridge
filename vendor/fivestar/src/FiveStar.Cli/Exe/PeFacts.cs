// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Formats.Common;
using FiveStar.Formats.Platform;

namespace FiveStar.Cli.Exe;

/// <summary>Builds the <see cref="PeBasics"/> of an image: sections with entropy, TLS callbacks, .pdata coverage and exports.</summary>
internal static class PeFacts
{
    // A RUNTIME_FUNCTION of x64 .pdata is 12 bytes (begin, end, unwind-info RVA).
    private const int RuntimeFunctionSize = 12;

    // A section this dense is most likely packed or encrypted rather than ordinary code or data.
    private const double HighEntropyBits = 7.2;
    private const int EntropyFloorBytes = 4096;

    private static readonly HashSet<string> StandardSections = new(StringComparer.Ordinal)
    {
        ".text", ".rdata", ".data", ".pdata", ".rsrc", ".reloc", ".tls", ".bss", ".idata", ".edata",
        ".didat", ".gfids", ".00cfg", ".voltbl", ".xdata", ".CRT", ".textbss",
    };

    public static PeBasics Build(PeImage image)
    {
        var sections = new List<SectionInfo>();

        foreach (var section in image.Sections)
        {
            sections.Add(Describe(image, section));
        }

        var (functions, bytes) = Pdata(image);

        return new PeBasics(sections, PeTls.Callbacks(image), functions, bytes, PeDetails.Exports(image));
    }

    private static SectionInfo Describe(PeImage image, PeSection section)
    {
        var start = (int)image.RawStart(section);
        var length = (int)Math.Max(0, Math.Min(section.RawSize, image.Data.Length - start));
        var clampedStart = (int)Math.Min(start, image.Data.Length);
        var entropy = ByteEntropy.BitsPerByte(image.Data.AsSpan(clampedStart, length));
        var high = entropy >= HighEntropyBits && section.RawSize >= EntropyFloorBytes;
        var console = !StandardSections.Contains(section.Name);

        return new SectionInfo(section.Name, section.VirtualAddress, section.VirtualSize, section.RawSize, entropy, high, console);
    }

    private static (int Functions, uint Bytes) Pdata(PeImage image)
    {
        var (rva, size) = image.Directory(PeDirectory.Exception);

        if (rva == 0)
        {
            return (0, 0);
        }

        return ((int)(size / RuntimeFunctionSize), size);
    }
}
