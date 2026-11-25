// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Collections.Concurrent;
using System.Globalization;
using System.Text.RegularExpressions;

namespace FiveStar.Shaders.Translation;

/// <summary>
/// An S# a program builds from literal constants (a root signature's static sampler, which the console's compiler bakes
/// into the code), for a sample or a comparison (<paramref name="Comparison"/>: a depth compare, which needs a
/// comparison sampler on the desktop). Its sampler heap slot follows from its words alone (<see cref="SlotOf"/>), so
/// every program, process and run agrees on it whatever order they meet the samplers in, and a program compiled once
/// stays right. A program names its samplers in its header (<see cref="Line"/>); the producers keep those lines in
/// <see cref="FileName"/> beside the DXIL, from which the loader makes the samplers.
/// </summary>
public sealed partial record StaticSampler(int Slot, IReadOnlyList<uint> Words, bool Comparison)
{
    /// <summary>The first slot of the static samplers (rdna_prelude STATIC_SAMPLER_BASE); the game's own fill the slots below.</summary>
    public const int FirstSlot = 1024;

    /// <summary>The slots the static samplers hash into, up to the end of the desktop sampler heap.</summary>
    public const int Slots = 1024;

    /// <summary>SQ_IMG_SAMP word 0 FORCE_UNNORMALIZED (bit 15) and FORCE_DEGAMMA (bit 20): fields the shader applies, not the sampler.</summary>
    public const uint ShaderSideBits = (1u << 15) | (1u << 20);

    /// <summary>The file of the static samplers of a folder's programs, one header line each.</summary>
    public const string FileName = "static-samplers.txt";

    private const string CompareTag = " compare";

    // The lines this process put in each folder's file already.
    private static readonly ConcurrentDictionary<string, bool> Kept = new(StringComparer.Ordinal);
    private const uint OffsetBasis = 2166136261;
    private const uint Prime = 16777619;
    private const int Dwords = 4;
    private const int ByteBits = 8;
    private const int WordBits = 32;
    private const uint ByteMask = 0xffu;

    /// <summary>The sampler of these S# words.</summary>
    public static StaticSampler Of(IReadOnlyList<uint> words, bool comparison)
    {
        return new StaticSampler(SlotOf(words, comparison), [.. words], comparison);
    }

    /// <summary>The slot of an S#: an FNV-1a hash of its words and use, into <see cref="Slots"/> slots from <see cref="FirstSlot"/>.</summary>
    public static int SlotOf(IReadOnlyList<uint> words, bool comparison)
    {
        var hash = OffsetBasis;
        var use = 0u;

        if (comparison)
        {
            use = 1u;
        }

        foreach (var word in words.Append(use))
        {
            for (var shift = 0; shift < WordBits; shift += ByteBits)
            {
                hash = unchecked((hash ^ ((word >> shift) & ByteMask)) * Prime);
            }
        }

        return FirstSlot + (int)(hash % Slots);
    }

    /// <summary>The program's header line: <c>// static sampler &lt;slot&gt;: S# = w0 w1 w2 w3[ compare]</c>.</summary>
    public string Line
    {
        get
        {
            var line = $"// static sampler {Slot}: S# = {string.Join(" ", Words.Select(word => word.ToString("x8", CultureInfo.InvariantCulture)))}";

            if (Comparison)
            {
                line += CompareTag;
            }

            return line;
        }
    }

    /// <summary>The static samplers a text names in header lines (a program, or a <see cref="FileName"/>).</summary>
    public static IReadOnlyList<StaticSampler> Read(string text)
    {
        var samplers = new List<StaticSampler>();

        foreach (Match match in LinePattern().Matches(text))
        {
            var words = match.Groups["words"].Value.Split(' ', StringSplitOptions.RemoveEmptyEntries).Select(word => uint.Parse(word, NumberStyles.HexNumber, CultureInfo.InvariantCulture)).ToArray();

            if (words.Length == Dwords)
            {
                samplers.Add(new StaticSampler(int.Parse(match.Groups["slot"].Value, CultureInfo.InvariantCulture), words, match.Groups["compare"].Success));
            }
        }

        return samplers;
    }

    /// <summary>
    /// Adds the static samplers a program's source names to <paramref name="folder"/>'s <see cref="FileName"/>, where its
    /// DXIL goes: the loader makes the samplers from there for programs it did not translate (the compile host's, a
    /// cache's, <c>fivestar setup</c>'s). Processes append to it side by side; a repeated line is harmless.
    /// </summary>
    public static void Keep(string folder, string hlsl)
    {
        var lines = Read(hlsl).Select(sampler => sampler.Line).Where(line => Kept.TryAdd(folder + "|" + line, true)).ToList();

        if (lines.Count == 0)
        {
            return;
        }

        using var file = new FileStream(Path.Combine(folder, FileName), FileMode.Append, FileAccess.Write, FileShare.ReadWrite);
        using var writer = new StreamWriter(file);
        writer.Write(string.Concat(lines.Select(line => line + "\n")));
    }

    /// <summary>Whether two samplers are the same S# for the same use.</summary>
    public bool SameAs(StaticSampler other)
    {
        return Slot == other.Slot && Comparison == other.Comparison && Words.SequenceEqual(other.Words);
    }

    [GeneratedRegex(@"^// static sampler (?<slot>\d+): S# = (?<words>[0-9a-f ]+?)(?<compare> compare)?\s*$", RegexOptions.Multiline)]
    private static partial Regex LinePattern();
}
