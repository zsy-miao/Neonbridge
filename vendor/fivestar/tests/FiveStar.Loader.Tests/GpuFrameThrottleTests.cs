// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Shims.D3D12X.Present;
using FiveStar.Loader.Tests.Support;
using Xunit;

namespace FiveStar.Loader.Tests;

/// <summary>A present waits for GPU work of earlier frames that has not finished, and goes on once it has.</summary>
public sealed unsafe class GpuFrameThrottleTests
{
    /// <summary>ID3D12Fence::Signal (d3d12.h).</summary>
    private const int FenceSignalSlot = 10;

    [Fact]
    public void A_wait_for_unfinished_gpu_work_holds_until_the_fence_reaches_it()
    {
        const uint ShortWait = 200;

        using var test = TestRuntime.Create();
        var device = new XboxDevice(test);
        nint fence = test.Runtime.D3D12X.Objects.Unwrap(device.Fence());
        using var throttle = new GpuFrameThrottle(ShortWait);

        // The work is held back: the fence stays at 0, so the wait runs its whole length and gives up.
        long started = Environment.TickCount64;
        Assert.Equal(GpuFrameThrottle.Outcome.GaveUp, throttle.WaitFor(fence, 1));
        Assert.True(Environment.TickCount64 - started >= ShortWait - 20);

        Assert.Equal(0, ((delegate* unmanaged<nint, ulong, int>)XboxDevice.Vtable(fence)[FenceSignalSlot])(fence, 1));
        Assert.Equal(GpuFrameThrottle.Outcome.Done, throttle.WaitFor(fence, 1));
    }
}
