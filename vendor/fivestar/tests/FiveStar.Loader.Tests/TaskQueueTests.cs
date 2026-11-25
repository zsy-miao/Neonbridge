// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Tests.Support;
using Xunit;

namespace FiveStar.Loader.Tests;

/// <summary>XTaskQueue through its native entry points: when and where callbacks run.</summary>
public sealed unsafe class TaskQueueTests
{
    [Fact]
    public void A_manual_queue_runs_callbacks_in_order_only_when_dispatched()
    {
        using var test = TestRuntime.Create();
        NativeCallbacks.Reset();
        var create = (delegate* unmanaged<uint, uint, nint*, int>)test.Gdk("XTaskQueueCreate");
        var submit = (delegate* unmanaged<nint, uint, nint, nint, int>)test.Gdk("XTaskQueueSubmitCallback");
        var dispatch = (delegate* unmanaged<nint, uint, uint, byte>)test.Gdk("XTaskQueueDispatch");
        delegate* unmanaged<nint, byte, void> record = &NativeCallbacks.Record;

        nint queue = 0;
        create(0, 0, &queue);
        submit(queue, 0, 1, (nint)record);
        submit(queue, 0, 2, (nint)record);
        submit(queue, 0, 3, (nint)record);
        bool ranEarly = !NativeCallbacks.Calls.IsEmpty;

        int dispatched = 0;

        while (dispatch(queue, 0, 0) != 0)
        {
            dispatched++;
        }

        var calls = NativeCallbacks.Drain();

        Assert.False(ranEarly);
        Assert.Equal(3, dispatched);
        Assert.Equal([1, 2, 3], calls.Select(c => c.Context));
        Assert.All(calls, c => Assert.Equal(Environment.CurrentManagedThreadId, c.Thread));
    }

    [Fact]
    public void Thread_pool_queues_run_every_callback_and_serialized_queues_one_at_a_time_in_order()
    {
        using var test = TestRuntime.Create();
        NativeCallbacks.Reset();
        var create = (delegate* unmanaged<uint, uint, nint*, int>)test.Gdk("XTaskQueueCreate");
        var submit = (delegate* unmanaged<nint, uint, nint, nint, int>)test.Gdk("XTaskQueueSubmitCallback");
        delegate* unmanaged<nint, byte, void> record = &NativeCallbacks.Record;
        delegate* unmanaged<nint, byte, void> slow = &NativeCallbacks.RecordSlowly;

        nint pool = 0;
        create(1, 1, &pool);

        for (int i = 0; i < 64; i++)
        {
            submit(pool, 0, 100 + i, (nint)record);
        }

        SpinWait.SpinUntil(() => NativeCallbacks.Calls.Count >= 64, 5000);
        var pooled = NativeCallbacks.Drain();

        nint serialized = 0;
        create(2, 2, &serialized);

        for (int i = 0; i < 20; i++)
        {
            submit(serialized, 0, 200 + i, (nint)slow);
        }

        SpinWait.SpinUntil(() => NativeCallbacks.Calls.Count >= 20, 5000);
        var ordered = NativeCallbacks.Drain();

        Assert.Equal(Enumerable.Range(100, 64).Select(i => (nint)i), pooled.Select(c => c.Context).Order());
        Assert.Equal(Enumerable.Range(200, 20).Select(i => (nint)i), ordered.Select(c => c.Context));
        Assert.Equal(1, NativeCallbacks.MaxConcurrency);
    }

    [Fact]
    public void Terminating_a_queue_delivers_pending_callbacks_as_canceled_and_refuses_new_ones()
    {
        using var test = TestRuntime.Create();
        NativeCallbacks.Reset();
        var create = (delegate* unmanaged<uint, uint, nint*, int>)test.Gdk("XTaskQueueCreate");
        var submit = (delegate* unmanaged<nint, uint, nint, nint, int>)test.Gdk("XTaskQueueSubmitCallback");
        var terminate = (delegate* unmanaged<nint, byte, nint, nint, int>)test.Gdk("XTaskQueueTerminate");

        delegate* unmanaged<nint, byte, void> record = &NativeCallbacks.Record;
        delegate* unmanaged<nint, void> terminated = &NativeCallbacks.QueueTerminated;

        nint queue = 0;
        create(0, 0, &queue);
        submit(queue, 0, 41, (nint)record);
        submit(queue, 0, 42, (nint)record);
        int hr = terminate(queue, 1, 55, (nint)terminated);
        int after = submit(queue, 0, 43, (nint)record);

        var calls = NativeCallbacks.Drain();

        Assert.Equal(0, hr);
        Assert.Equal([41, 42], calls.Select(c => c.Context));
        Assert.All(calls, c => Assert.True(c.Canceled));
        Assert.Equal([55], NativeCallbacks.Terminated);
        Assert.Equal(unchecked((int)0x80004004), after);
    }
}
