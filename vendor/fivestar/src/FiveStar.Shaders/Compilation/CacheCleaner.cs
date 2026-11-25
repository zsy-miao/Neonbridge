// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Compilation;

/// <summary>
/// Measures and removes shader-cache entries the current identity does not use: what older translator, LLVM or dxc
/// builds left behind, entries made before usage was recorded, and temporary files of interrupted writes.
/// </summary>
public static class CacheCleaner
{
    /// <summary>Splits the cache below <paramref name="root"/> by whether the identity's usage record lists an entry.</summary>
    public static CacheSweep Measure(string root, string identity)
    {
        return Sweep(root, identity, remove: false);
    }

    /// <summary>Removes the entries and usage records <see cref="Measure"/> counts as stale; returns what it found.</summary>
    public static CacheSweep Clean(string root, string identity)
    {
        return Sweep(root, identity, remove: true);
    }

    private static CacheSweep Sweep(string root, string identity, bool remove)
    {
        if (!Directory.Exists(root))
        {
            return new CacheSweep(0, 0, 0, 0, 0);
        }

        var used = CacheUsage.Load(root, identity);
        var current = (Entries: 0, Bytes: 0L);
        var stale = (Entries: 0, Bytes: 0L);

        foreach (var file in EntryFiles(root))
        {
            var name = Path.GetRelativePath(root, file.FullName).Replace(Path.DirectorySeparatorChar, '/');

            if (used.Contains(name))
            {
                current = (current.Entries + 1, current.Bytes + file.Length);
                continue;
            }

            stale = (stale.Entries + 1, stale.Bytes + file.Length);

            if (remove)
            {
                file.Delete();
            }
        }

        var others = OtherRecords(root, identity, remove);

        if (remove)
        {
            RemoveEmptyFolders(root);
        }

        return new CacheSweep(current.Entries, current.Bytes, stale.Entries, stale.Bytes, others);
    }

    /// <summary>Every file in the entry layout <c>&lt;kind&gt;/&lt;shard&gt;/&lt;file&gt;</c>; other files are never touched.</summary>
    private static IEnumerable<FileInfo> EntryFiles(string root)
    {
        var records = Path.GetFullPath(Path.Combine(root, CacheUsage.FolderName));

        foreach (var kind in new DirectoryInfo(root).EnumerateDirectories())
        {
            if (string.Equals(kind.FullName, records, StringComparison.OrdinalIgnoreCase))
            {
                continue;
            }

            foreach (var file in kind.EnumerateDirectories().SelectMany(shard => shard.EnumerateFiles()))
            {
                yield return file;
            }
        }
    }

    private static int OtherRecords(string root, string identity, bool remove)
    {
        var own = Path.GetFullPath(CacheUsage.PathOf(root, identity));
        var folder = new DirectoryInfo(Path.Combine(root, CacheUsage.FolderName));

        if (!folder.Exists)
        {
            return 0;
        }

        var others = folder.EnumerateFiles().Where(file => !string.Equals(file.FullName, own, StringComparison.OrdinalIgnoreCase)).ToList();

        if (remove)
        {
            others.ForEach(file => file.Delete());
        }

        return others.Count;
    }

    private static void RemoveEmptyFolders(string root)
    {
        var folders = Directory.EnumerateDirectories(root, "*", SearchOption.AllDirectories).OrderByDescending(folder => folder.Length).ToList();

        foreach (var folder in folders)
        {
            if (!Directory.EnumerateFileSystemEntries(folder).Any())
            {
                Directory.Delete(folder);
            }
        }
    }
}
