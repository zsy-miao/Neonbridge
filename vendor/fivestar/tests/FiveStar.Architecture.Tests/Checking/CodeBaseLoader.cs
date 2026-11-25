// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Text;

namespace FiveStar.Architecture.Tests.Checking;

/// <summary>Reads the repository from disk into a <see cref="CodeBase"/>, skipping build output and tool folders.</summary>
public static class CodeBaseLoader
{
    private const string SolutionFile = "FiveStar.slnx";
    private const string GitMarker = ".git";
    private const int BinaryProbeLength = 8000;
    private const char ByteOrderMark = '﻿';

    /// <summary>The repository root: the first folder at or above <paramref name="start"/> that holds the solution.</summary>
    public static string FindRoot(string start)
    {
        var folder = new DirectoryInfo(start);

        while (folder is not null && !File.Exists(Path.Combine(folder.FullName, SolutionFile)))
        {
            folder = folder.Parent;
        }

        if (folder is null)
        {
            throw new DirectoryNotFoundException($"No {SolutionFile} at or above {start}.");
        }

        return folder.FullName;
    }

    public static CodeBase Load(string root)
    {
        var files = new List<SourceFile>();
        Collect(new DirectoryInfo(root), root, files);

        return new CodeBase(files);
    }

    private static void Collect(DirectoryInfo folder, string root, List<SourceFile> files)
    {
        foreach (var file in folder.EnumerateFiles())
        {
            var path = Path.GetRelativePath(root, file.FullName).Replace('\\', '/');
            files.Add(new SourceFile(path, ReadText(file.FullName)));
        }

        foreach (var child in folder.EnumerateDirectories())
        {
            if (CodePolicy.SkippedFolders.Contains(child.Name) || IsSeparateCheckout(child))
            {
                continue;
            }

            Collect(child, root, files);
        }
    }

    /// <summary>A nested repository or git worktree: its own <c>.git</c> file or folder means it is not part of this one.</summary>
    private static bool IsSeparateCheckout(DirectoryInfo folder)
    {
        var marker = Path.Combine(folder.FullName, GitMarker);

        return File.Exists(marker) || Directory.Exists(marker);
    }

    /// <summary>The file's text, or null when a NUL byte near its start marks it as binary.</summary>
    private static string? ReadText(string path)
    {
        var bytes = File.ReadAllBytes(path);
        var probe = bytes.AsSpan(0, Math.Min(bytes.Length, BinaryProbeLength));

        if (probe.Contains((byte)0))
        {
            return null;
        }

        return Encoding.UTF8.GetString(bytes).TrimStart(ByteOrderMark);
    }
}
