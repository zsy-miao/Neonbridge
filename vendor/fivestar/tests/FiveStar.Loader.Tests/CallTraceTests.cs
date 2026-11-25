// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using FiveStar.Loader.Codegen;
using FiveStar.Loader.Diagnostics;
using Xunit;

namespace FiveStar.Loader.Tests;

/// <summary>The call-trace thunks of --run: arguments reach the target unchanged and the ring keeps the newest calls.</summary>
public sealed unsafe class CallTraceTests
{
    [Fact]
    public void Wrapped_calls_keep_their_arguments_and_are_recorded_in_order()
    {
        var heap = new CodeHeap();
        var trace = new CallTrace(heap);
        delegate* unmanaged<long, long, long, long, long, long, long> target = &Weigh;
        var first = (delegate* unmanaged<long, long, long, long, long, long, long>)trace.Wrap((nint)target, "a!First");
        var second = (delegate* unmanaged<long, long, long, long, long, long, long>)trace.Wrap((nint)target, "a!Second");
        heap.Seal();

        long result = first(1, 2, 3, 4, 5, 6);

        for (int i = 0; i < 300; i++)
        {
            _ = second(0, 0, 0, 0, 0, 0);
        }

        _ = first(0, 0, 0, 0, 0, 0);

        // Two stack arguments (the fifth and sixth) pass through the jump.
        Assert.Equal(123456, result);
        Assert.Equal(302UL, trace.CallCount);
        Assert.Equal(["a!Second", "a!Second", "a!First"], trace.Recent(3));
        Assert.Equal(256, trace.Recent(1000).Count);
        Assert.Equal(2UL, trace.Totals()["a!First"]);
        Assert.Equal(300UL, trace.Totals()["a!Second"]);
    }

    [UnmanagedCallersOnly]
    private static long Weigh(long a, long b, long c, long d, long e, long f)
    {
        return (a * 100000) + (b * 10000) + (c * 1000) + (d * 100) + (e * 10) + f;
    }
}
