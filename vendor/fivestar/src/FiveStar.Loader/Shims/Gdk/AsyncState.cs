// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.Gdk;

/// <summary>The state of one XAsyncBlock between XAsyncBegin and its cleanup.</summary>
internal sealed class AsyncState
{
    private volatile int _status = HResult.Pending;

    public AsyncState(nint block, nint provider, nint context, nint identity, string? identityName, TaskQueue queue)
    {
        Block = block;
        Provider = provider;
        Context = context;
        Identity = identity;
        IdentityName = identityName;
        Queue = queue;
    }

    public nint Block { get; }

    /// <summary>The <c>HRESULT XAsyncProvider(XAsyncOp op, const XAsyncProviderData* data)</c>.</summary>
    public nint Provider { get; }

    public nint Context { get; }

    /// <summary>The identity XAsyncGetResult checks the caller against.</summary>
    public nint Identity { get; }

    public string? IdentityName { get; }

    public TaskQueue Queue { get; }

    public object Gate { get; } = new();

    /// <summary>Set when the completion routine has run (or there is none).</summary>
    public ManualResetEventSlim Done { get; } = new(false);

    public int Status
    {
        get => _status;
        set => _status = value;
    }

    public nuint RequiredSize { get; set; }

    public bool CleanedUp { get; set; }

    public bool ResultTaken { get; set; }

    public bool WorkStarted { get; set; }

    public bool CancelRequested { get; set; }

    /// <summary>The completion routine read from the block when the call completed.</summary>
    public nint CompletionRoutine { get; set; }

    public AsyncStep? WorkStep { get; set; }

    public AsyncStep? CompletionStep { get; set; }
}
