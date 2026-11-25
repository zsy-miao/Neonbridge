// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Cache;

/// <summary>
/// File names read from cache files and joined to a folder: a name must name a file in that folder, never a path out
/// of it (rooted, a drive, a UNC name, ".." or any separator).
/// </summary>
public static class PlainFileName
{
    private static readonly char[] Separators = ['/', '\\', ':'];

    /// <summary>Whether <paramref name="name"/> is a plain file name.</summary>
    public static bool Is(string? name)
    {
        if (string.IsNullOrWhiteSpace(name) || name is "." or ".." || name.Contains("..", StringComparison.Ordinal))
        {
            return false;
        }

        return name.IndexOfAny(Separators) < 0 && name.IndexOfAny(Path.GetInvalidFileNameChars()) < 0 && !Path.IsPathRooted(name)
            && name == Path.GetFileName(name);
    }

    /// <summary>
    /// <paramref name="folder"/> joined with <paramref name="name"/>, checked to stay inside the folder; throws
    /// <see cref="InvalidDataException"/> otherwise.
    /// </summary>
    public static string Inside(string folder, string name)
    {
        var root = Path.GetFullPath(folder);
        var target = Path.GetFullPath(Path.Combine(root, name));
        var prefix = Path.EndsInDirectorySeparator(root) ? root : root + Path.DirectorySeparatorChar;

        if (!PlainFileName.Is(name) || !target.StartsWith(prefix, StringComparison.OrdinalIgnoreCase))
        {
            throw new InvalidDataException($"not a file of {folder}: {name}");
        }

        return target;
    }
}
