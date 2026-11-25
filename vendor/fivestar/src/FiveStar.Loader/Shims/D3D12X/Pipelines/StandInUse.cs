// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.D3D12X.Pipelines;

/// <summary>
/// How much the game misses a stand-in: the frames in which a command list skipped its work and the draws and
/// dispatches skipped. Its build's place in the urgent queue follows from it (<see cref="Score"/>): what is drawn
/// every frame first, then what is drawn most.
/// </summary>
internal sealed class StandInUse
{
    /// <summary>Draws counted beyond this do not order builds any further.</summary>
    private const long DrawCap = (1L << DrawBits) - 1;
    private const int DrawBits = 24;

    private static long _frame;
    private long _draws;
    private long _frames;
    private long _lastFrame = -1;

    /// <summary>The present count, which the presenter advances.</summary>
    public static long Frame
    {
        get => Volatile.Read(ref _frame);
        set => Volatile.Write(ref _frame, value);
    }

    /// <summary>Frames that skipped the stand-in's work.</summary>
    public long Frames => Volatile.Read(ref _frames);

    /// <summary>Draws and dispatches skipped.</summary>
    public long Draws => Volatile.Read(ref _draws);

    /// <summary>The build's priority: frames first, then draws.</summary>
    public long Score => (Frames << DrawBits) | Math.Min(Draws, DrawCap);

    /// <summary>A command list skipped the stand-in's work in <paramref name="frame"/>.</summary>
    public void Skipped(long frame)
    {
        Interlocked.Increment(ref _draws);

        if (Volatile.Read(ref _lastFrame) != frame && Interlocked.Exchange(ref _lastFrame, frame) != frame)
        {
            Interlocked.Increment(ref _frames);
        }
    }
}
