// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;
using System.Reflection;
using FiveStar.Shaders.Compilation;

namespace FiveStar.Cli.Setup;

/// <summary>
/// Fingerprint parts: dump files by path, size and write time (the dump is far too large to hash on every run), small
/// files by content, and assemblies by content (builds are deterministic, so the same source gives the same hash).
/// </summary>
internal static class Fingerprints
{
    private const string Missing = "missing";

    /// <summary>The files' paths below <paramref name="root"/>, sizes and write times; a missing file counts as missing.</summary>
    public static string OfFiles(string root, IEnumerable<string> paths)
    {
        var parts = new List<string>();

        foreach (var path in paths)
        {
            var file = new FileInfo(path);
            var name = Path.GetRelativePath(root, path);

            if (!file.Exists)
            {
                parts.Add($"{name}:{Missing}");
                continue;
            }

            parts.Add(string.Create(CultureInfo.InvariantCulture, $"{name}:{file.Length}:{file.LastWriteTimeUtc.Ticks}"));
        }

        return ContentHash.OfText([.. parts]);
    }

    /// <summary>The bytes of a small file; its absence counts as missing.</summary>
    public static string OfContent(string path)
    {
        if (!File.Exists(path))
        {
            return Missing;
        }

        return ContentHash.Of(File.ReadAllBytes(path));
    }

    /// <summary>The build of the assemblies that declare the types.</summary>
    public static string OfAssemblies(params ReadOnlySpan<Type> types)
    {
        var paths = new List<string>();

        foreach (var type in types)
        {
            paths.Add(type.Assembly.Location);
        }

        return ContentHash.OfFiles(paths);
    }

    public static string Combine(params ReadOnlySpan<string> parts)
    {
        return ContentHash.OfText(parts);
    }

    /// <summary>The version of the <c>fivestar</c> command, for the setup header.</summary>
    public static string CommandVersion()
    {
        var assembly = typeof(Fingerprints).Assembly;
        var version = assembly.GetCustomAttribute<AssemblyInformationalVersionAttribute>();

        if (version is null)
        {
            return assembly.GetName().Version?.ToString() ?? string.Empty;
        }

        return version.InformationalVersion;
    }
}
