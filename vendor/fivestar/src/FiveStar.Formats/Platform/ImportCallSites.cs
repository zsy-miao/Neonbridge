// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Buffers.Binary;

namespace FiveStar.Formats.Platform;

/// <summary>
/// Counts the call sites of each import: the <c>call qword [rip + disp32]</c> and <c>jmp qword [rip + disp32]</c>
/// instructions in the code sections whose RIP-relative target is an import address table slot. This is the usual
/// form a compiler emits for an imported call and its tail call, so the count is a close measure of how often the
/// game reaches each function.
/// </summary>
public static class ImportCallSites
{
    private const string CodeSectionPrefix = ".text";
    private const byte MemoryGroupOpcode = 0xFF;
    private const byte CallRipModRm = 0x15;
    private const byte JmpRipModRm = 0x25;
    private const int InstructionLength = 6;
    private const int DisplacementOffset = 2;

    /// <summary>Call-site counts for every imported function, in the order the imports list them.</summary>
    public static IReadOnlyList<ImportCallSite> Count(PeImage image, IReadOnlyList<PeImport> imports)
    {
        var references = References(image);
        var pointerSize = image.Is64Bit ? sizeof(ulong) : sizeof(uint);
        var sites = new List<ImportCallSite>();

        foreach (var import in imports)
        {
            AddModule(import, references, pointerSize, sites);
        }

        return sites;
    }

    private static void AddModule(PeImport import, IReadOnlyDictionary<uint, int> references, int pointerSize, List<ImportCallSite> sites)
    {
        for (var index = 0; index < import.Functions.Count; index++)
        {
            var slotRva = (uint)(import.FirstThunkRva + (index * pointerSize));
            var count = references.GetValueOrDefault(slotRva, 0);
            sites.Add(new ImportCallSite(import.Dll, import.Functions[index], import.Delayed, slotRva, count));
        }
    }

    /// <summary>Maps each referenced IAT slot RVA to the number of calls and jumps that reach it.</summary>
    private static Dictionary<uint, int> References(PeImage image)
    {
        var counts = new Dictionary<uint, int>();

        foreach (var section in image.Sections.Where(section => section.Name.StartsWith(CodeSectionPrefix, StringComparison.Ordinal)))
        {
            ScanSection(image, section, counts);
        }

        return counts;
    }

    private static void ScanSection(PeImage image, PeSection section, Dictionary<uint, int> counts)
    {
        var start = (int)image.RawStart(section);
        var length = (int)Math.Min(section.RawSize, section.VirtualSize);
        var available = Math.Min(length, image.Data.Length - start);

        if (available <= 0)
        {
            return;
        }

        var code = image.Data.AsSpan(start, available);

        for (var at = 0; at + InstructionLength <= code.Length; at++)
        {
            CountAt(section, code, at, counts);
        }
    }

    private static void CountAt(PeSection section, ReadOnlySpan<byte> code, int at, Dictionary<uint, int> counts)
    {
        if (code[at] != MemoryGroupOpcode)
        {
            return;
        }

        var modrm = code[at + 1];

        if (modrm != CallRipModRm && modrm != JmpRipModRm)
        {
            return;
        }

        var displacement = BinaryPrimitives.ReadInt32LittleEndian(code[(at + DisplacementOffset)..]);
        var target = (uint)(section.VirtualAddress + at + InstructionLength + displacement);
        counts[target] = counts.GetValueOrDefault(target, 0) + 1;
    }
}
