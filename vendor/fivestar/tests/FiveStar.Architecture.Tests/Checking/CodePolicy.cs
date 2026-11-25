// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Collections.Frozen;

namespace FiveStar.Architecture.Tests.Checking;

/// <summary>The repository's settings for the rules of CONTRIBUTING.md.</summary>
public static class CodePolicy
{
    public const int MaxMethodLines = 60;

    public const int MaxStatementsPerGroup = 6;

    public const string TestProjectSuffix = ".Tests";

    /// <summary>The two header lines, without their comment marker.</summary>
    public static readonly IReadOnlyList<string> HeaderLines =
    [
        "Copyright (c) 2026 Five Star Recomp contributors.",
        "SPDX-License-Identifier: GPL-3.0-or-later",
    ];

    /// <summary>The code files by extension, with the line comment marker of their language.</summary>
    public static readonly FrozenDictionary<string, string> CommentMarkers = new Dictionary<string, string>
    {
        [".cs"] = "//",
        [".hlsl"] = "//",
        [".hlsli"] = "//",
        [".ps1"] = "#",
        [".py"] = "#",
        [".sh"] = "#",
    }.ToFrozenDictionary(StringComparer.Ordinal);

    /// <summary>Folders that hold build output or tool state rather than sources.</summary>
    public static readonly FrozenSet<string> SkippedFolders =
        new[] { "bin", "obj", "artifacts", ".git", ".vs", "TestResults" }.ToFrozenSet(StringComparer.OrdinalIgnoreCase);

    /// <summary>Words no path or line may contain, in any letter case. Built from parts so this file does not hold them.</summary>
    public static readonly IReadOnlyList<string> ForbiddenWords =
    [
        string.Concat("cla", "ude"),
        string.Concat("chat", "gpt"),
        string.Concat("co", "pilot"),
        string.Concat("co-authored", "-by"),
        string.Concat("generated", " with"),
        string.Concat("anth", "ropic"),
        string.Concat("open", "ai"),
        string.Concat("as an", " ai"),
    ];

    /// <summary>Projects that may read the clock, the environment and randomness directly.</summary>
    public static readonly IReadOnlyList<string> AmbientStateProjects = ["FiveStar.Cli", "FiveStar.Loader", "FiveStar.Debugger", "*.Tests"];

    /// <summary>Projects whose literals are their data.</summary>
    public static readonly IReadOnlyList<string> MagicNumberExemptProjects = ["*.Tests"];

    /// <summary>
    /// Attributes of a source generator that needs a partial type; the last three are the loader's shim registry
    /// (FiveStar.Loader.Generators).
    /// </summary>
    public static readonly FrozenSet<string> GeneratorAttributes =
        new[] { "GeneratedRegex", "JsonSerializable", "LibraryImport", "Shim", "ShimModule", "NotImplementedTable" }.ToFrozenSet(StringComparer.Ordinal);

    /// <summary>The projects of each layer and the projects they may reference. A test project references only its subject.</summary>
    public static readonly FrozenDictionary<string, string[]> Layers = new Dictionary<string, string[]>
    {
        ["FiveStar.Formats"] = [],
        ["FiveStar.Shaders"] = [],
        ["FiveStar.Loader"] = ["FiveStar.Formats", "FiveStar.Shaders", "FiveStar.Graphics", "FiveStar.Loader.Generators"],
        ["FiveStar.Loader.Generators"] = [],
        ["FiveStar.Graphics"] = [],
        ["FiveStar.Cli"] = ["FiveStar.Formats", "FiveStar.Graphics", "FiveStar.Shaders"],
        ["FiveStar.Benchmarks"] = ["FiveStar.Formats", "FiveStar.Loader"],
        ["FiveStar.Debugger"] = [],
    }.ToFrozenDictionary(StringComparer.Ordinal);

    /// <summary>
    /// Member attributes of a framework that subclasses the declaring class at run time, so the class may not be
    /// sealed (BenchmarkDotNet generates a derived type per benchmark class).
    /// </summary>
    public static readonly FrozenSet<string> UnsealedFrameworkAttributes =
        new[] { "Benchmark" }.ToFrozenSet(StringComparer.Ordinal);

    /// <summary>The attribute, by name in any namespace, that marks a method running every frame or tick.</summary>
    public const string HotPathAttribute = "HotPath";

    /// <summary>
    /// Types a <c>[HotPath]</c> method may create with <c>new</c> besides the structs and enums the repository declares
    /// and the runtime's own value types.
    /// </summary>
    public static readonly FrozenSet<string> HotPathValueTypes = FrozenSet<string>.Empty;

    /// <summary>Whether a project name matches one of the patterns; a pattern may start with <c>*</c>.</summary>
    public static bool Matches(string? project, IReadOnlyList<string> patterns)
    {
        if (project is null)
        {
            return false;
        }

        return patterns.Any(pattern => Matches(project, pattern));
    }

    private static bool Matches(string project, string pattern)
    {
        if (pattern.StartsWith('*'))
        {
            return project.EndsWith(pattern[1..], StringComparison.Ordinal);
        }

        return project == pattern;
    }
}
