// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Native;
using FiveStar.Loader.Shims.D3D12X.Pipelines;
using Xunit;

namespace FiveStar.Loader.Tests;

/// <summary>A creation a game thread runs at first use: one the compile budget would make it wait for goes back to the build threads.</summary>
public sealed class PipelineBuildQueueTests
{
    private static readonly TimeSpan BuildLimit = TimeSpan.FromSeconds(30);

    [Fact]
    public async Task A_creation_that_may_not_start_on_a_game_thread_goes_back_to_the_build_threads()
    {
        var queue = new PipelineBuildQueue(1, PipelineBuildQueue.AnyTime);
        bool? noWaitSeen = null;
        var build = queue.Queue(() =>
        {
            noWaitSeen ??= CompileBudget.NoWait;

            // On the game thread the budget has no room; on a build thread it builds.
            if (CompileBudget.NoWait)
            {
                return new PipelineResult(HResult.Pending, 0);
            }

            return new PipelineResult(HResult.Ok, 1);
        }, urgent: false);

        Assert.False(queue.RunNow(build));
        Assert.True(noWaitSeen);
        Assert.False(CompileBudget.NoWait);
        Assert.False(build.Done.Task.IsCompleted);

        queue.Start();

        var result = await build.Done.Task.WaitAsync(BuildLimit);
        Assert.Equal(HResult.Ok, result.Hr);
    }

    [Fact]
    public async Task A_creation_no_command_list_waits_for_gives_its_thread_back_instead_of_waiting_for_room()
    {
        var queue = new PipelineBuildQueue(1, PipelineBuildQueue.AnyTime);
        int attempts = 0;
        var order = new List<string>();
        var speculative = queue.Queue(() =>
        {
            // The budget has no room for it the first time: it goes back to the queue, the thread takes the urgent one.
            if (CompileBudget.GiveBack && Interlocked.Increment(ref attempts) == 1)
            {
                Thread.Sleep(100);

                return new PipelineResult(HResult.Pending, 0);
            }

            lock (order)
            {
                order.Add("speculative");
            }

            return new PipelineResult(HResult.Ok, 1);
        }, urgent: false);

        queue.Start();
        SpinWait.SpinUntil(() => Volatile.Read(ref attempts) == 1, BuildLimit);

        var urgent = queue.Queue(() =>
        {
            lock (order)
            {
                order.Add("urgent");
            }

            return new PipelineResult(HResult.Ok, 2);
        }, urgent: true);

        await Task.WhenAll(speculative.Done.Task, urgent.Done.Task).WaitAsync(BuildLimit);
        Assert.Equal(["urgent", "speculative"], order);
    }

    [Fact]
    public void The_warm_up_reports_what_is_left_and_the_rate()
    {
        Assert.Equal("300/1200 after 00:10:00, 900 left at 0.50/s (about 00:30:00)", PipelineWarmup.Progress(300, 1200, TimeSpan.FromMinutes(10)));
        Assert.Equal("0/10 after 00:00:00, 10 left at 0.00/s (about unknown)", PipelineWarmup.Progress(0, 10, TimeSpan.Zero));
    }

    [Fact]
    public void Urgent_creations_go_by_the_frames_then_the_draws_their_stand_ins_missed()
    {
        var queue = new PipelineBuildQueue(1, PipelineBuildQueue.AnyTime);
        var once = new StandInUse();
        var everyFrame = new StandInUse();
        var heavyOnce = new StandInUse();

        once.Skipped(1);
        heavyOnce.Skipped(1);
        heavyOnce.Skipped(1);
        heavyOnce.Skipped(1);

        for (long frame = 1; frame <= 3; frame++)
        {
            everyFrame.Skipped(frame);
        }

        var speculative = queue.Queue(() => new PipelineResult(HResult.Ok, 0), urgent: false);
        var first = queue.Queue(() => new PipelineResult(HResult.Ok, 1), urgent: true, once);
        var drawn = queue.Queue(() => new PipelineResult(HResult.Ok, 2), urgent: true, heavyOnce);
        var framed = queue.Queue(() => new PipelineResult(HResult.Ok, 3), urgent: true, everyFrame);

        // Three frames beat three draws in one frame, which beat one draw; the speculative one comes last.
        Assert.Same(framed, queue.Take(speculative: true));
        Assert.Same(drawn, queue.Take(speculative: true));
        Assert.Same(first, queue.Take(speculative: true));
        Assert.Same(speculative, queue.Take(speculative: true));
    }
}
