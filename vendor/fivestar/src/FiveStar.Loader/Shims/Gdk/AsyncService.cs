// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Collections.Concurrent;
using System.Runtime.InteropServices;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.Gdk;

/// <summary>
/// XAsync on the task queues (XAsync.h, XAsyncProvider.h). DoWork runs on the work port; a DoWork result other than
/// E_PENDING completes the call; the completion routine runs on the completion port; Cleanup runs exactly once (on
/// completion when there is no result or it failed, else after GetResult); XAsyncGetStatus(wait) returns after the
/// completion routine ran. The other GDK shims run their *Async functions through <see cref="BeginManaged"/>.
/// </summary>
internal sealed unsafe class AsyncService
{
    private readonly ConcurrentDictionary<nint, AsyncState> _states = new();
    private readonly Dictionary<string, nint> _identities = [];
    private readonly TaskQueueService _taskQueues;
    private readonly NativeBoundary _boundary;

    public AsyncService(TaskQueueService taskQueues, NativeBoundary boundary)
    {
        _taskQueues = taskQueues;
        _boundary = boundary;
    }

    public AsyncState? State(nint block)
    {
        if (block == 0)
        {
            return null;
        }

        return _states.GetValueOrDefault(block);
    }

    public int Begin(nint block, nint context, nint identity, string? identityName, nint provider)
    {
        if (block == 0 || provider == 0)
        {
            return HResult.InvalidArgument;
        }

        var previous = State(block);

        if (previous is not null && previous.Status == HResult.Pending)
        {
            _boundary.Warn($"XAsyncBegin: XAsyncBlock 0x{block:x} is still in use by {previous.IdentityName}");

            return HResult.InvalidArgument;
        }

        // No queue on the block and the process queue was cleared.
        var queue = _taskQueues.ResolveQueue(*(nint*)(block + AsyncLayout.BlockQueueOffset));

        if (queue is null)
        {
            return HResult.InvalidArgument;
        }

        var state = CreateState(block, provider, context, identity, identityName, queue);
        int hr = CallProvider(state, AsyncOperation.Begin, 0, 0);

        if (HResult.Failed(hr))
        {
            _boundary.Trace($"XAsyncBegin({identityName}) Begin failed {HResult.Format(hr)}");
            state.Status = hr;
            Cleanup(state);
            state.Done.Set();

            return hr;
        }

        return HResult.Ok;
    }

    [HotPath]
    public int Schedule(nint block, uint delayMs)
    {
        var state = State(block);

        if (state is null)
        {
            return HResult.InvalidArgument;
        }

        return state.Queue.Work.Submit(new TaskEntry(0, 0, state.WorkStep), state.Queue, delayMs);
    }

    /// <summary>The work step: DoWork, unless the call was canceled first.</summary>
    public void RunWork(AsyncState state, bool canceled)
    {
        if (state.Status != HResult.Pending)
        {
            return;
        }

        if (canceled || state.CancelRequested)
        {
            Complete(state.Block, HResult.Abort, 0);

            return;
        }

        lock (state.Gate)
        {
            state.WorkStarted = true;
        }

        // A result other than E_PENDING completes the call (ignored when the provider already completed it).
        int hr = CallProvider(state, AsyncOperation.DoWork, 0, 0);

        if (hr != HResult.Pending)
        {
            Complete(state.Block, hr, 0);
        }
    }

    public void Complete(nint block, int result, nuint requiredSize)
    {
        var state = State(block);

        if (state is null)
        {
            return;
        }

        if (result == HResult.Pending)
        {
            _boundary.Warn("XAsyncComplete with E_PENDING ignored");

            return;
        }

        if (!Record(state, result, requiredSize))
        {
            return;
        }

        if (HResult.Failed(result) || state.RequiredSize == 0)
        {
            Cleanup(state);
        }

        Notify(state);
    }

    [HotPath]
    public int GetStatus(nint block, bool wait)
    {
        var state = State(block);

        if (state is null)
        {
            return HResult.InvalidArgument;
        }

        // A completed call is done for the waiter: the completion routine itself may wait on its own block (the game
        // frees the block in its routine after XAsyncGetStatus(block, true)), and Done is set only after the routine.
        if (wait && state.Status == HResult.Pending)
        {
            state.Done.Wait();
        }

        return state.Status;
    }

    [HotPath]
    public int GetResultSize(nint block, nuint* size)
    {
        var state = State(block);

        if (state is null || size == null)
        {
            return HResult.InvalidArgument;
        }

        if (state.Status == HResult.Pending || HResult.Failed(state.Status))
        {
            return state.Status;
        }

        *size = state.RequiredSize;

        return HResult.Ok;
    }

    public int GetResult(nint block, nint identity, nuint bufferSize, nint buffer, nuint* used)
    {
        var state = State(block);

        if (state is null)
        {
            return HResult.InvalidArgument;
        }

        if (state.Status == HResult.Pending)
        {
            return HResult.Pending;
        }

        if (identity != state.Identity)
        {
            _boundary.Warn($"XAsyncGetResult: identity mismatch (block belongs to {state.IdentityName})");

            return HResult.InvalidArgument;
        }

        if (HResult.Failed(state.Status))
        {
            return state.Status;
        }

        if (used != null)
        {
            *used = 0;
        }

        if (state.RequiredSize == 0)
        {
            return state.Status;
        }

        lock (state.Gate)
        {
            if (state.ResultTaken)
            {
                return HResult.IllegalMethodCall;
            }

            if (buffer == 0 || bufferSize < state.RequiredSize)
            {
                return HResult.NotSufficientBuffer;
            }

            state.ResultTaken = true;
        }

        return TakeResult(state, buffer, used);
    }

    public void Cancel(nint block)
    {
        var state = State(block);

        if (state is null || state.Status != HResult.Pending)
        {
            return;
        }

        bool started;

        lock (state.Gate)
        {
            state.CancelRequested = true;
            started = state.WorkStarted;
        }

        // A provider that handles Cancel completes with E_ABORT itself; otherwise, or before DoWork started, complete here.
        int hr = CallProvider(state, AsyncOperation.Cancel, 0, 0);

        if (state.Status == HResult.Pending && (HResult.Failed(hr) || !started))
        {
            Complete(block, HResult.Abort, 0);
        }
    }

    /// <summary>XAsyncRun(block, work): the work callback runs as DoWork; its HRESULT is the call's result.</summary>
    public int Run(nint block, nint work)
    {
        delegate* unmanaged<uint, nint*, int> provider = &AsyncProviders.RunProvider;

        return Begin(block, work, (nint)provider, "XAsyncRun", (nint)provider);
    }

    /// <summary>Starts a shim-side async operation for <paramref name="api"/> on <paramref name="block"/>.</summary>
    public int BeginManaged(nint block, string api, ManagedAsyncOperation operation)
    {
        var handle = GCHandle.Alloc(operation);
        delegate* unmanaged<uint, nint*, int> provider = &AsyncProviders.ManagedProvider;
        int hr = Begin(block, GCHandle.ToIntPtr(handle), Identity(api), api, (nint)provider);
        _boundary.Trace($"{api}(block 0x{block:x}) -> {HResult.Format(hr)}");

        return hr;
    }

    /// <summary>The *Result half of a managed operation started for <paramref name="api"/>.</summary>
    public int ManagedResult(nint block, string api, nuint size, nint buffer)
    {
        return GetResult(block, Identity(api), size, buffer, null);
    }

    /// <summary>A stable identity pointer per API name, which XAsyncGetResult checks.</summary>
    public nint Identity(string api)
    {
        lock (_identities)
        {
            if (!_identities.TryGetValue(api, out nint identity))
            {
                identity = (nint)NativeMemory.AllocZeroed((nuint)sizeof(nint));
                _identities[api] = identity;
            }

            return identity;
        }
    }

    /// <summary>Calls the provider with an XAsyncProviderData on the stack.</summary>
    public int CallProvider(AsyncState state, AsyncOperation operation, nint buffer, nuint size)
    {
        nint* data = stackalloc nint[AsyncLayout.ProviderDataSlots];
        data[AsyncLayout.DataAsyncSlot] = state.Block;
        data[AsyncLayout.DataBufferSizeSlot] = (nint)size;
        data[AsyncLayout.DataBufferSlot] = buffer;
        data[AsyncLayout.DataContextSlot] = state.Context;

        return ((delegate* unmanaged<uint, nint*, int>)state.Provider)((uint)operation, data);
    }

    private AsyncState CreateState(nint block, nint provider, nint context, nint identity, string? identityName, TaskQueue queue)
    {
        var state = new AsyncState(block, provider, context, identity, identityName, queue);
        state.WorkStep = new AsyncWorkStep(this, state);
        state.CompletionStep = new AsyncCompletionStep(state);
        new Span<byte>((void*)(block + AsyncLayout.BlockInternalOffset), AsyncLayout.BlockInternalSize).Clear();
        *(ulong*)(block + AsyncLayout.BlockInternalOffset) = AsyncLayout.InternalMarker;
        _states[block] = state;

        return state;
    }

    /// <summary>Records the result once; false when the call already completed.</summary>
    private bool Record(AsyncState state, int result, nuint requiredSize)
    {
        lock (state.Gate)
        {
            if (state.Status != HResult.Pending)
            {
                return false;
            }

            state.RequiredSize = HResult.Failed(result) ? 0 : requiredSize;
            state.Status = result;
        }

        _boundary.Trace($"XAsyncComplete({state.IdentityName}) -> {HResult.Format(result)}, result size {state.RequiredSize}");

        return true;
    }

    /// <summary>Queues the completion routine on the completion port; runs it inline when the queue is terminated.</summary>
    private void Notify(AsyncState state)
    {
        state.CompletionRoutine = *(nint*)(state.Block + AsyncLayout.BlockCallbackOffset);

        if (state.CompletionRoutine == 0)
        {
            state.Done.Set();

            return;
        }

        var queue = state.Queue;

        if (queue.Completion.Submit(new TaskEntry(0, 0, state.CompletionStep), queue, 0) != HResult.Ok)
        {
            state.CompletionStep!.Run(false);
        }
    }

    private int TakeResult(AsyncState state, nint buffer, nuint* used)
    {
        int hr = CallProvider(state, AsyncOperation.GetResult, buffer, state.RequiredSize);

        if (used != null && !HResult.Failed(hr))
        {
            *used = state.RequiredSize;
        }

        Cleanup(state);

        return hr;
    }

    private void Cleanup(AsyncState state)
    {
        lock (state.Gate)
        {
            if (state.CleanedUp)
            {
                return;
            }

            state.CleanedUp = true;
        }

        CallProvider(state, AsyncOperation.Cleanup, 0, 0);
    }
}
