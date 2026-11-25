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

    /// <summary>The marker of a program that does not trace rays.</summary>
    public const string NoMarkerExtension = ".rt0";

    /// <summary>What a program that traces rays includes.</summary>
    private const string BvhInclude = "rdna_bvh.hlsli";

    private static readonly ConcurrentDictionary<string, bool> Known = new(StringComparer.Ordinal);
    private static readonly ConcurrentDictionary<string, byte> Logged = new(StringComparer.Ordinal);

    /// <summary>Whether ray-tracing pipelines stay stand-ins (default on; off with <c>--raytracing</c>).</summary>
    public static bool StandIns { get; set; } = true;

    /// <summary>The program cache folder the markers are in; null before programs are made on demand.</summary>
    public static string? Folder { get; set; }

    /// <summary>
    /// Reads whether a program traces rays from its console code (<see cref="ProgramProducer"/>: an image_bvh
    /// instruction), for a program whose DXIL a boot before the markers made: w66b's held
    /// standard_emissive_layer~CS_RaytracingResolve, found compiled, took one of the four driver compile slots. Null when
    /// the program cannot be read.
    /// </summary>
    public static Func<string, bool?>? Scanner { get; set; }

    /// <summary>
    /// The markers' folder for a program folder (<c>&lt;program cache&gt;/&lt;identity&gt;/&lt;adapter&gt;</c>):
    /// <c>&lt;program cache&gt;/ray-tracing</c>, which every identity shares. Whether a console program traces rays does not
    /// change with the translator, and markers kept per identity were lost at each new one, so every boot after an update
    /// read programs again (boots w66d to w69).
    /// </summary>
    public static string MarkerFolder(string programFolder)
    {
        string root = Path.GetDirectoryName(Path.GetDirectoryName(programFolder)) ?? programFolder;
        string folder = Path.Combine(root, "ray-tracing");
        Directory.CreateDirectory(folder);

        return folder;
    }

    /// <summary>Records whether the program <paramref name="name"/> translated to <paramref name="hlsl"/> traces rays, with its marker.</summary>
    public static void Note(string name, string hlsl, string folder)
    {
        Record(name, Traces(hlsl), folder);
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

    /// <summary>
    /// True when the pipeline of <paramref name="names"/> traces rays and stays a stand-in; the first time, logged.
    /// Known programs and markers only, so a game thread creating a pipeline reads no program.
    /// </summary>
    public static bool Skips(string names)
    {
        return Skips(names, Folder, scanner: null);
    }

    /// <summary>
    /// As <see cref="Skips(string)"/>, reading markers in <paramref name="folder"/> and, with <paramref name="scan"/>, the
    /// console code of a program without one (before a build or a pipeline-cache load: nothing traces rays unseen).
    /// </summary>
    public static bool Skips(string names, string? folder, bool scan)
    {
        Func<string, bool?>? scanner = null;

        if (scan)
        {
            scanner = Scanner;
        }

        return Skips(names, folder, scanner);
    }

    /// <summary>As <see cref="Skips(string, string?, bool)"/> with the given <paramref name="scanner"/> (null: markers only).</summary>
    public static bool Skips(string names, string? folder, Func<string, bool?>? scanner)
    {
        if (!StandIns)
        {
            return false;
        }

        foreach (Match program in ProgramPattern().Matches(names))
        {
            if (!Is(program.Value, folder, scanner))
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
    /// Whether a program traces rays: noted, or its marker in the program cache (<c>.rt</c> traces, <c>.rt0</c> does
    /// not), or by <paramref name="scanner"/> from its console code (then marked). Unknown without one reads as not.
    /// </summary>
    private static bool Is(string program, string? folder, Func<string, bool?>? scanner)
    {
        if (Known.TryGetValue(program, out bool known))
        {
            return known;
        }

        bool? marked = Marked(program, folder);

        if (marked is bool value)
        {
            Known[program] = value;

            return value;
        }

        if (scanner?.Invoke(program) is not bool traces)
        {
            return false;
        }

        Record(program, traces, folder);

        return traces;
    }

    /// <summary>A program's marker: true for <c>.rt</c>, false for <c>.rt0</c>, null for none (or no folder).</summary>
    private static bool? Marked(string program, string? folder)
    {
        if (folder is null)
        {
            return null;
        }

        if (File.Exists(Path.Combine(folder, program + MarkerExtension)))
        {
            return true;
        }

        if (File.Exists(Path.Combine(folder, program + NoMarkerExtension)))
        {
            return false;
        }

        return null;
    }

    /// <summary>Records what a program does and its marker (<c>.rt</c> or <c>.rt0</c>) when there is a folder.</summary>
    private static void Record(string name, bool traces, string? folder)
    {
        Known[name] = traces;

        if (folder is null)
        {
            return;
        }

        string extension = NoMarkerExtension;

        if (traces)
        {
            extension = MarkerExtension;
        }

        try
        {
            File.WriteAllBytes(Path.Combine(folder, name + extension), []);
        }
        catch (IOException)
        {
            // Another process wrote it.
        }
    }

    /// <summary>A program's name in a pipeline's names (which add link and trim suffixes and " + ").</summary>
    [GeneratedRegex(@"sgd\d_[0-9a-f]{8}_t[0-9a-f]+_\S*?_Wrapped")]
    private static partial Regex ProgramPattern();
}
