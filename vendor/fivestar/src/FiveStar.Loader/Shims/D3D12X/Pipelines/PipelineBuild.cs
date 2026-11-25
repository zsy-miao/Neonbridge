// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.D3D12X.Pipelines;

/// <summary>
/// One queued desktop pipeline creation (<see cref="PipelineBuildQueue"/>) and where its result goes. A build promoted
/// to urgent sits in both queues; the first thread to claim it runs it.
/// </summary>
internal sealed class PipelineBuild
{
    private int _claimed;
    private int _urgent;

    private StandInUse? _use;

    public PipelineBuild(Func<PipelineResult> create, bool urgent, StandInUse? use)
    {
        Create = create;
        _urgent = urgent ? 1 : 0;
        _use = use;
    }

    /// <summary>How much the game misses the stand-in this build replaces; null for a build with none (yet).</summary>
    public StandInUse? Use
    {
        get => Volatile.Read(ref _use);
        set => Volatile.Write(ref _use, value);
    }

    /// <summary>The build's place among urgent ones: higher first (<see cref="StandInUse.Score"/>).</summary>
    public long Score => Use?.Score ?? 0;

    public Func<PipelineResult> Create { get; }

    /// <summary>Whether a command list waits for it (queued urgent or promoted since).</summary>
    public bool Urgent => Volatile.Read(ref _urgent) == 1;

    public TaskCompletionSource<PipelineResult> Done { get; } = new(TaskCreationOptions.RunContinuationsAsynchronously);

    /// <summary>True for the one caller that runs the build.</summary>
    public bool Claim()
    {
        return Interlocked.Exchange(ref _claimed, 1) == 0;
    }

    /// <summary>Gives a claimed build back unbuilt, so a build thread can claim it.</summary>
    public void Release()
    {
        Volatile.Write(ref _claimed, 0);
    }

    /// <summary>True for the first call on a build that was not urgent yet: it then needs an urgent queue entry.</summary>
    public bool MakeUrgent()
    {
        return Interlocked.Exchange(ref _urgent, 1) == 0 && Volatile.Read(ref _claimed) == 0;
    }
}
