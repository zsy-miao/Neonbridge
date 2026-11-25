// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Host;
using Xunit;

namespace FiveStar.Loader.Tests;

/// <summary>The split warm-up: how many processes, which pipelines each builds, their arguments and the run's progress.</summary>
public sealed class WarmShardsTests
{
    private const ulong Gigabyte = 1UL << 30;

    [Fact]
    public void Shards_cover_every_pipeline_once_and_get_the_parents_arguments_with_their_own()
    {
        int[] owners = Enumerable.Range(0, 10).Select(position => Enumerable.Range(0, 3).Single(shard => WarmShards.InShard(position, shard, 3))).ToArray();
        string[] parent = ["--warm-pipelines", "shaders.rpf", "--warm-jobs", "12", "--log", "warm.log", "--warm-used-only", "--warm-processes", "3"];

        var child = WarmShards.ChildArguments(parent, 1, 3, 4, 4, "warm.log");

        Assert.Equal([0, 1, 2, 0, 1, 2, 0, 1, 2, 0], owners);
        Assert.Equal(["--warm-pipelines", "shaders.rpf", "--warm-used-only", "--warm-shard", "1/3", "--warm-jobs", "4", "--max-driver-compiles", "4", "--log", "warm.log.shard1.log"], child);
        Assert.Equal((1, 3), WarmShards.ParseShard("1/3"));
        Assert.Null(WarmShards.ParseShard("3/3"));
        Assert.Equal(3, WarmShards.ProcessesFor(63 * Gigabyte));
        Assert.Equal(1, WarmShards.ProcessesFor(16 * Gigabyte - 1));
    }

    [Fact]
    public void The_run_progress_sums_each_shards_last_line()
    {
        var progress = new (int Done, int Total)[2];

        WarmShards.Note("  warm-up 100/1400 after 00:10:00, 1300 left at 0.17/s (about 02:10:00): 20 built", 0, progress);
        WarmShards.Note("  warm-up 300/1400 after 00:10:00, 1100 left at 0.50/s (about 00:36:40): 60 built", 1, progress);
        WarmShards.Note("[d3d12_xs] something else", 1, progress);

        Assert.Equal("warm-up shards: 400/2800 after 00:10:00, 2400 left at 0.67/s (about 01:00:00)", WarmShards.Overall(progress, TimeSpan.FromMinutes(10)));
    }
}
