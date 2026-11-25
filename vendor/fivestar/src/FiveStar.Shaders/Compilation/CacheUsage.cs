// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Compilation;

/// <summary>
/// The cache entries one identity (a translator and compiler build) has used: <c>&lt;root&gt;/used/&lt;identity&gt;.txt</c>,
/// one entry name per line. Lines are appended as entries are used, so an interrupted run keeps its record. Entries no
/// usage record of the current identity lists belong to older builds and can be removed (<see cref="CacheCleaner"/>).
/// Safe to call from several threads.
/// </summary>
public sealed class CacheUsage : IDisposable
{
    /// <summary>The folder below the cache root that holds the usage records.</summary>
    public const string FolderName = "used";

    private const string RecordExtension = ".txt";

    private readonly HashSet<string> _entries;
    private readonly StreamWriter _writer;
    private readonly Lock _gate = new();

    /// <summary>Opens the record of <paramref name="identity"/> below <paramref name="root"/>, keeping what it already lists.</summary>
    public CacheUsage(string root, string identity)
    {
        var path = PathOf(root, identity);
        _entries = Load(root, identity);
        Directory.CreateDirectory(Path.GetDirectoryName(path)!);
        _writer = new StreamWriter(path, append: true) { AutoFlush = true };
    }

    /// <summary>The path of an identity's usage record.</summary>
    public static string PathOf(string root, string identity)
    {
        return Path.Combine(root, FolderName, identity + RecordExtension);
    }

    /// <summary>The entry names an identity's record lists; empty when it has none.</summary>
    public static HashSet<string> Load(string root, string identity)
    {
        var path = PathOf(root, identity);
        var entries = new HashSet<string>(StringComparer.Ordinal);

        if (!File.Exists(path))
        {
            return entries;
        }

        foreach (var line in File.ReadLines(path))
        {
            if (line.Length > 0)
            {
                entries.Add(line);
            }
        }

        return entries;
    }

    public void Record(string entry)
    {
        lock (_gate)
        {
            if (_entries.Add(entry))
            {
                _writer.WriteLine(entry);
            }
        }
    }

    public void Dispose()
    {
        lock (_gate)
        {
            _writer.Dispose();
        }
    }
}
