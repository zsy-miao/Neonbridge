// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Collections.Concurrent;
using System.Text.RegularExpressions;

namespace FiveStar.Loader.Shims.D3D12X.Pipelines;

/// <summary>
/// Without <c>--raytracing</c> no acceleration structure is built, and a pipeline whose programs trace rays
/// (image_bvh_intersect_ray, so their HLSL includes rdna_bvh.hlsli) stays a stand-in: never built nor loaded from the
/// pipeline cache, its draws and dispatches skipped, logged once, as <see cref="SkippedPipelines"/> does. Boot w65b,
/// whose node reads all missed (<see cref="Binding.BvhMiss"/>), still lost the device about 100 s after the world
/// loaded; with these pipelines skipped boots ran 300 to 600 s (VIctory holds such pipelines until its builder has
/// run, its PR #2311; facts only). A program is known to trace rays when it is translated (<see cref="Note"/>), and
/// across boots and from the compile host by a marker beside its DXIL in the program cache, <c>&lt;name&gt;.rt</c>.
/// </summary>
internal static partial class RayTracingPrograms
{
    /// <summary>The marker's extension, beside <c>&lt;name&gt;.dxil</c>.</summary>
    public const string MarkerExtension = ".rt";

    /// <summary>What a program that traces rays includes.</summary>
    private const string BvhInclude = "rdna_bvh.hlsli";

    private static readonly ConcurrentDictionary<string, bool> Known = new(StringComparer.Ordinal);
    private static readonly ConcurrentDictionary<string, byte> Logged = new(StringComparer.Ordinal);

    /// <summary>Whether ray-tracing pipelines stay stand-ins (default on; off with <c>--raytracing</c>).</summary>
    public static bool StandIns { get; set; } = true;

    /// <summary>The program cache folder the markers are in; null before programs are made on demand.</summary>
    public static string? Folder { get; set; }

    /// <summary>Records whether the program <paramref name="name"/> translated to <paramref name="hlsl"/> traces rays, with its marker.</summary>
    public static void Note(string name, string hlsl, string folder)
    {
        bool traces = Traces(hlsl);
        Known[name] = traces;

        if (!traces)
        {
            return;
        }

        try
        {
            File.WriteAllBytes(Path.Combine(folder, name + MarkerExtension), []);
        }
        catch (IOException)
        {
            // Another process wrote it.
        }
    }

    /// <summary>Reads the marker of a program another process made (the compile host) again.</summary>
    public static void Refresh(string name)
    {
        Known.TryRemove(name, out _);
    }

    /// <summary>Whether translated HLSL traces rays.</summary>
    public static bool Traces(string hlsl)
    {
        return hlsl.Contains(BvhInclude, StringComparison.Ordinal);
    }

    /// <summary>True when the pipeline of <paramref name="names"/> traces rays and stays a stand-in; the first time, logged.</summary>
    public static bool Skips(string names)
    {
        return Skips(names, Folder);
    }

    /// <summary>As <see cref="Skips(string)"/>, reading markers in <paramref name="folder"/>.</summary>
    public static bool Skips(string names, string? folder)
    {
        if (!StandIns)
        {
            return false;
        }

        foreach (Match program in ProgramPattern().Matches(names))
        {
            if (!Is(program.Value, folder))
            {
                continue;
            }

            if (Logged.TryAdd(names, 0))
            {
                ShimContext.Log?.Warn($"[d3d12_xs] pipeline {names} traces rays ({program.Value}) and no acceleration structure is built without --raytracing: it stays a stand-in and its work is skipped");
            }

            return true;
        }

        return false;
    }

    /// <summary>
    /// Whether a program traces rays: noted, or its marker is in the program cache. A program not made yet reads as not;
    /// making it notes it (<see cref="Note"/>, or <see cref="Refresh"/> for the compile host's).
    /// </summary>
    private static bool Is(string program, string? folder)
    {
        return Known.GetOrAdd(program, name => folder is not null && File.Exists(Path.Combine(folder, name + MarkerExtension)));
    }

    /// <summary>A program's name in a pipeline's names (which add link and trim suffixes and " + ").</summary>
    [GeneratedRegex(@"sgd\d_[0-9a-f]{8}_t[0-9a-f]+_\S*?_Wrapped")]
    private static partial Regex ProgramPattern();
}
