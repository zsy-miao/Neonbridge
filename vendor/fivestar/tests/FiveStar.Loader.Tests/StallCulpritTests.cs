// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Shims.D3D12X;
using Xunit;

namespace FiveStar.Loader.Tests;

/// <summary>A stall names its culprit: the queue asked to signal the awaited fence value and the work it had first.</summary>
public sealed class StallCulpritTests
{
    private const nint Fence = 0x1000;
    private const nint Other = 0x2000;
    private const nint Skin = 0x51;
    private const nint Clear = 0x52;

    [Fact]
    public void A_wait_names_the_queue_that_signals_its_value_and_the_unfinished_work_before_it()
    {
        QueueEvent[] events =
        [
            Execute(10, Clear),
            Signal(Fence, 4),
            Wait(Other, 7),
            Execute(11, Clear),
            Execute(12, Skin),
            Signal(Fence, 5),
            Execute(13, Clear),
        ];

        // The game waits for the fence to reach 5: queue #1 signals it after executions 12 and 11; 11 finished, 12 not.
        var text = StallCulprit.Describe(1, events, Fence, 5, 11, Name);

        Assert.NotNull(text);
        Assert.StartsWith("queue #1 signals it to 5 after", text, StringComparison.Ordinal);
        Assert.Contains("execute #12 NOT DONE: 0 draws, 0 dispatches, 0 indirect; last pipeline MSNC_VS_LinearDepthSkin", text, StringComparison.Ordinal);
        Assert.Contains("execute #11 done", text, StringComparison.Ordinal);
        Assert.Contains("after a Wait for fence 0x2000 to 7", text, StringComparison.Ordinal);
        Assert.True(text.IndexOf("#12", StringComparison.Ordinal) < text.IndexOf("#11", StringComparison.Ordinal));

        // A value no queue was asked to signal yet: none names it; the last Signal is known.
        Assert.Null(StallCulprit.Describe(1, events, Fence, 6, 11, Name));
        Assert.Equal(5UL, StallCulprit.LastSignal(events, Fence));
        Assert.Null(StallCulprit.LastSignal(events, 0x3000));
    }

    private static string Name(nint pipeline)
    {
        if (pipeline == Skin)
        {
            return "MSNC_VS_LinearDepthSkin";
        }

        return "PS_Clear";
    }

    private static QueueEvent Execute(ulong value, nint pipeline)
    {
        return new QueueEvent { Kind = QueueEventKind.Execute, Value = value, Pipeline = pipeline, Lists = 1 };
    }

    private static QueueEvent Signal(nint fence, ulong value)
    {
        return new QueueEvent { Kind = QueueEventKind.Signal, Fence = fence, Value = value };
    }

    private static QueueEvent Wait(nint fence, ulong value)
    {
        return new QueueEvent { Kind = QueueEventKind.Wait, Fence = fence, Value = value };
    }
}
