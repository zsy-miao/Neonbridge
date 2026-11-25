// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;
using FiveStar.Shaders.Tests.Warp;

namespace FiveStar.Shaders.Tests.Differential;

/// <summary>Compares every output word of two runs: render targets, writable buffers and textures, indirect arguments.</summary>
public static class OutputComparison
{
    /// <summary>Null when the runs agree; otherwise the number of differing words and the first one.</summary>
    public static string? Describe(WarpResult first, WarpResult second)
    {
        var differences = new List<string>();
        var count = 0;

        for (var target = 0; target < first.Targets.Count; target++)
        {
            count += Compare($"target {target}", first.Targets[target], second.Targets[target], differences);
        }

        foreach (var (slot, words) in first.Buffers)
        {
            count += Compare($"buffer {slot}", words, second.Buffers[slot], differences);
        }

        foreach (var (slot, words) in first.Textures)
        {
            count += Compare($"texture {slot}", words, second.Textures[slot], differences);
        }

        if (first.IndirectArguments is not null)
        {
            count += Compare("arguments", first.IndirectArguments, second.IndirectArguments!, differences);
        }

        if (count == 0)
        {
            return null;
        }

        return string.Create(CultureInfo.InvariantCulture, $"{count} words differ: {string.Join("; ", differences)}");
    }

    /// <summary>
    /// The output words a run changed: render-target words off the clear value, buffer and texture words off their input
    /// (zero for buffers the inputs do not list).
    /// </summary>
    public static int Written(WarpResult result, Bindings inputs)
    {
        var written = result.Targets.Sum(target => target.Count(word => word != Framebuffer.ClearUint));

        foreach (var (slot, words) in result.Buffers)
        {
            var initial = inputs.Buffers.FirstOrDefault(buffer => buffer.HeapIndex == slot)?.Words ?? [];
            written += words.Where((word, index) => word != Initial(initial, index)).Count();
        }

        foreach (var texture in inputs.Textures.Where(texture => result.Textures.ContainsKey(texture.HeapIndex)))
        {
            written += result.Textures[texture.HeapIndex].Where((word, index) => word != texture.Words[index]).Count();
        }

        return written;
    }

    private static uint Initial(uint[] words, int index)
    {
        if (index < words.Length)
        {
            return words[index];
        }

        return 0;
    }

    /// <summary>The number of differing words; the first difference of the resource goes to <paramref name="differences"/>.</summary>
    private static int Compare(string resource, uint[] first, uint[] second, List<string> differences)
    {
        var count = 0;

        for (var index = 0; index < first.Length; index++)
        {
            if (first[index] == second[index])
            {
                continue;
            }

            if (count == 0)
            {
                differences.Add(string.Create(CultureInfo.InvariantCulture, $"{resource} word {index}: {first[index]:x8} vs {second[index]:x8}"));
            }

            count++;
        }

        return count;
    }
}
