// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Cli.Exe;

/// <summary>
/// Finds the <c>fivestar-loader</c> executable: the <c>--loader</c> path, then <c>FIVESTAR_LOADER</c>, then the folder
/// of this CLI (a published build), then the sibling build output (both projects build under the same
/// <c>artifacts/bin</c> tree).
/// </summary>
internal static class LoaderLocator
{
    private const string EnvironmentVariable = "FIVESTAR_LOADER";
    private const string LoaderExe = "fivestar-loader.exe";
    private const string LoaderProject = "FiveStar.Loader";
    private const string CliProject = "FiveStar.Cli";

    public static string? Locate(string? requested)
    {
        if (requested is not null)
        {
            return Existing(requested);
        }

        var fromEnvironment = Environment.GetEnvironmentVariable(EnvironmentVariable);

        if (fromEnvironment is not null)
        {
            return Existing(fromEnvironment);
        }

        var beside = Path.Combine(AppContext.BaseDirectory, LoaderExe);

        if (File.Exists(beside))
        {
            return beside;
        }

        return Sibling();
    }

    /// <summary>The loader beside this CLI: <c>.../bin/FiveStar.Loader/&lt;config&gt;/fivestar-loader.exe</c>.</summary>
    private static string? Sibling()
    {
        var cliConfigDir = AppContext.BaseDirectory.TrimEnd(Path.DirectorySeparatorChar, Path.AltDirectorySeparatorChar);
        var config = Path.GetFileName(cliConfigDir);
        var cliProjectDir = Path.GetDirectoryName(cliConfigDir);

        if (cliProjectDir is null || !string.Equals(Path.GetFileName(cliProjectDir), CliProject, StringComparison.Ordinal))
        {
            return null;
        }

        var binDir = Path.GetDirectoryName(cliProjectDir);

        if (binDir is null)
        {
            return null;
        }

        return InLoaderTree(Path.Combine(binDir, LoaderProject), config);
    }

    /// <summary>The loader exe under its project's build tree; its leaf may be <c>&lt;config&gt;_&lt;rid&gt;</c>, so a config match is preferred.</summary>
    private static string? InLoaderTree(string loaderDir, string config)
    {
        if (!Directory.Exists(loaderDir))
        {
            return null;
        }

        var candidates = Directory.GetDirectories(loaderDir)
            .Where(directory => File.Exists(Path.Combine(directory, LoaderExe)))
            .OrderByDescending(directory => Path.GetFileName(directory).StartsWith(config, StringComparison.Ordinal))
            .ToList();

        if (candidates.Count == 0)
        {
            return null;
        }

        return Path.Combine(candidates[0], LoaderExe);
    }

    private static string? Existing(string path)
    {
        if (File.Exists(path))
        {
            return path;
        }

        return null;
    }
}
