// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Compilation;
using FiveStar.Shaders.Disassembly;
using FiveStar.Shaders.Translation;

namespace FiveStar.Shaders.Tests.Differential;

/// <summary>
/// A fixed pseudo-random sample of the programs of one stage: the folder's .fssm files ordered by a hash of their
/// name (so the sample spreads over the whole corpus and is the same on every run), the first <c>count</c> of the stage.
/// </summary>
public static class ProgramSample
{
    private const ulong OffsetBasis = 14695981039346656037;
    private const ulong Prime = 1099511628211;

    public static IReadOnlyList<string> Of(string folder, ShaderStage stage, int count)
    {
        var loader = new ProgramLoader(new RdnaDecoder());
        var stages = new HashSet<ShaderStage> { stage };
        var sample = new List<string>();

        foreach (var path in Directory.GetFiles(folder, "*.fssm").OrderBy(path => Hash(Path.GetFileName(path))))
        {
            if (sample.Count == count)
            {
                break;
            }

            if (loader.Load(path, stages).Status != ProgramStatus.SkipStage)
            {
                sample.Add(path);
            }
        }

        return sample;
    }

    private static ulong Hash(string name)
    {
        var hash = OffsetBasis;

        foreach (var character in name)
        {
            hash = unchecked((hash ^ character) * Prime);
        }

        return hash;
    }
}
