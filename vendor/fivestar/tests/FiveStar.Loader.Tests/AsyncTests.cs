// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using FiveStar.Loader.Tests.Support;
using Xunit;

namespace FiveStar.Loader.Tests;

/// <summary>XAsync through its native entry points: the provider sequence, results, cancellation and XAsyncRun.</summary>
public sealed unsafe class AsyncTests
{
    private const int Pending = unchecked((int)0x8000000A);
    private const int InvalidArgument = unchecked((int)0x80070057);
    private const int NotSufficientBuffer = unchecked((int)0x8007007A);
    private const int Abort = unchecked((int)0x80004004);

    [Fact]
    public void A_provider_sees_begin_dowork_getresult_cleanup_and_the_completion_routine_runs_on_its_port()
    {
        using var test = TestRuntime.Create();
        NativeCallbacks.Reset();
        nint queue = ManualQueue(test);
        nint identity = (nint)NativeMemory.AllocZeroed(8);
        nint block = TestNative.AsyncBlock(queue, (nint)(delegate* unmanaged<nint, void>)&NativeCallbacks.AsyncCompleted);
        var begin = (delegate* unmanaged<nint, nint, nint, byte*, nint, int>)test.Gdk("XAsyncBegin");

        var status = (delegate* unmanaged<nint, byte, int>)test.Gdk("XAsyncGetStatus");
        var result = (delegate* unmanaged<nint, nint, nuint, nint, nuint*, int>)test.Gdk("XAsyncGetResult");
        var dispatch = (delegate* unmanaged<nint, uint, uint, byte>)test.Gdk("XTaskQueueDispatch");

        int hr = begin(block, 0, identity, null, (nint)(delegate* unmanaged<uint, nint*, int>)&NativeCallbacks.TestProvider);
        int beforeWork = status(block, 0);
        dispatch(queue, 0, 0);
        bool completedBeforeDispatch = !NativeCallbacks.Completed.IsEmpty;
        dispatch(queue, 1, 0);

        uint value = 0;
        nuint used = 0;
        int wrongIdentity = result(block, 1234, 4, (nint)(&value), &used);
        int tooSmall = result(block, identity, 2, (nint)(&value), &used);
        int ok = result(block, identity, 4, (nint)(&value), &used);

        Assert.Equal(0, hr);
        Assert.Equal(Pending, beforeWork);
        Assert.False(completedBeforeDispatch);
        Assert.Equal([block], NativeCallbacks.Completed);
        Assert.Equal(InvalidArgument, wrongIdentity);
        Assert.Equal(NotSufficientBuffer, tooSmall);

        Assert.Equal(0, ok);
        Assert.Equal(0xDEADBEEFu, value);
        Assert.Equal(4u, (uint)used);
        Assert.Equal([0u, 1u, 2u, 4u], NativeCallbacks.ProviderOperations);
    }

    [Fact]
    public void Canceling_before_the_work_ran_completes_with_abort()
    {
        using var test = TestRuntime.Create();
        NativeCallbacks.Reset();
        nint queue = ManualQueue(test);
        nint block = TestNative.AsyncBlock(queue, 0);
        var begin = (delegate* unmanaged<nint, nint, nint, byte*, nint, int>)test.Gdk("XAsyncBegin");
        var cancel = (delegate* unmanaged<nint, void>)test.Gdk("XAsyncCancel");

        var status = (delegate* unmanaged<nint, byte, int>)test.Gdk("XAsyncGetStatus");

        // A 10 s delay keeps DoWork from running before the cancel.
        begin(block, 10000, (nint)NativeMemory.AllocZeroed(8), null, (nint)(delegate* unmanaged<uint, nint*, int>)&NativeCallbacks.TestProvider);
        cancel(block);
        int final = status(block, 1);

        Assert.Equal(Abort, final);
        Assert.Equal([0u, 3u, 4u], NativeCallbacks.ProviderOperations);
    }

    [Fact]
    public void XAsyncRun_runs_the_work_on_the_process_queue_and_returns_its_result()
    {
        using var test = TestRuntime.Create();
        NativeCallbacks.Reset();
        nint block = TestNative.AsyncBlock(0, (nint)(delegate* unmanaged<nint, void>)&NativeCallbacks.AsyncCompleted);
        var run = (delegate* unmanaged<nint, nint, int>)test.Gdk("XAsyncRun");
        var status = (delegate* unmanaged<nint, byte, int>)test.Gdk("XAsyncGetStatus");

        int hr = run(block, (nint)(delegate* unmanaged<nint, int>)&NativeCallbacks.RunWork);
        int final = status(block, 1);

        Assert.Equal(0, hr);
        Assert.Equal(0x1234, final);
        Assert.Single(NativeCallbacks.Calls);
        Assert.Equal([block], NativeCallbacks.Completed);
    }

    [Fact]
    public void A_completion_routine_can_wait_on_its_own_block()
    {
        using var test = TestRuntime.Create();
        NativeCallbacks.Reset();
        NativeCallbacks.Status = (delegate* unmanaged<nint, byte, int>)test.Gdk("XAsyncGetStatus");
        nint block = TestNative.AsyncBlock(0, (nint)(delegate* unmanaged<nint, void>)&NativeCallbacks.AsyncCompletedWaiting);
        var run = (delegate* unmanaged<nint, nint, int>)test.Gdk("XAsyncRun");

        run(block, (nint)(delegate* unmanaged<nint, int>)&NativeCallbacks.RunWork);
        bool completed = SpinWait.SpinUntil(() => !NativeCallbacks.Completed.IsEmpty, TimeSpan.FromSeconds(5));

        Assert.True(completed);
    }

    private static nint ManualQueue(TestRuntime test)
    {
        nint queue = 0;
        ((delegate* unmanaged<uint, uint, nint*, int>)test.Gdk("XTaskQueueCreate"))(0, 0, &queue);

        NativeCallbacks.Complete = (delegate* unmanaged<nint, int, nuint, void>)test.Gdk("XAsyncComplete");
        NativeCallbacks.Schedule = (delegate* unmanaged<nint, uint, int>)test.Gdk("XAsyncSchedule");

        return queue;
    }
}
