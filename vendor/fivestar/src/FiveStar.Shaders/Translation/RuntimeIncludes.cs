// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Translation;

/// <summary>
/// The HLSL headers every translated program includes: rdna_prelude.hlsli (memory, descriptors, cross-lane and
/// conversion helpers) and rdna_bvh.hlsli (the BVH ray intersection). They are written next to the generated HLSL.
/// </summary>
public static class RuntimeIncludes
{
    /// <summary>The file names of the headers.</summary>
    public static IReadOnlyList<string> Names { get; } = ["rdna_prelude.hlsli", "rdna_bvh.hlsli"];

    /// <summary>The text of one header.</summary>
    public static string Read(string name)
    {
        using var stream = typeof(RuntimeIncludes).Assembly.GetManifestResourceStream(name)
            ?? throw new FileNotFoundException("runtime header not embedded", name);
        using var reader = new StreamReader(stream);

        return reader.ReadToEnd();
    }

    /// <summary>Writes every header into a folder.</summary>
    public static void WriteTo(string folder)
    {
        Directory.CreateDirectory(folder);

        foreach (var name in Names)
        {
            File.WriteAllText(Path.Combine(folder, name), Read(name));
        }
    }
}
