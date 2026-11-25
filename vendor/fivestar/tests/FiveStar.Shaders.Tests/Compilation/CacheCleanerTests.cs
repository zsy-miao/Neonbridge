// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Compilation;
using Xunit;

namespace FiveStar.Shaders.Tests.Compilation;

/// <summary>Cleaning the shader cache keeps every entry the current build used, also ones an older build wrote first.</summary>
public sealed class CacheCleanerTests : IDisposable
{
    private readonly DirectoryInfo _folder = Directory.CreateTempSubdirectory("fivestar-cache-");

    public void Dispose()
    {
        _folder.Delete(recursive: true);
    }

    [Fact]
    public void Cleaning_removes_only_the_entries_the_current_build_did_not_use()
    {
        var root = _folder.FullName;
        File.WriteAllText(Path.Combine(root, "notes.txt"), "not an entry");

        Use(root, "old-build", cache =>
        {
            cache.Write("dxil", "aa01", ".dxil", [1, 2, 3]);
            cache.Write("dxil", "bb02", ".dxil", [4, 5, 6]);
        });

        Use(root, "new-build", cache =>
        {
            Assert.NotNull(cache.Read("dxil", "aa01", ".dxil"));
            cache.Write("programs", "cc03", ".json", [7]);
        });

        var sweep = CacheCleaner.Clean(root, "new-build");

        Assert.Equal((2, 1, 1), (sweep.CurrentEntries, sweep.StaleEntries, sweep.OtherRecords));
        Assert.NotNull(new CacheFolder(root).Read("dxil", "aa01", ".dxil"));
        Assert.Null(new CacheFolder(root).Read("dxil", "bb02", ".dxil"));
        Assert.True(File.Exists(Path.Combine(root, "notes.txt")));
        Assert.Equal((2, 0), (CacheCleaner.Measure(root, "new-build").CurrentEntries, CacheCleaner.Measure(root, "new-build").StaleEntries));
    }

    private static void Use(string root, string identity, Action<CacheFolder> work)
    {
        using var usage = new CacheUsage(root, identity);
        var cache = new CacheFolder(root);
        cache.RecordUsage(usage);
        work(cache);
    }
}
