// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.Gdk;

/// <summary>Runs the block's completion routine on the completion port.</summary>
internal sealed class AsyncCompletionStep : AsyncStep
{
    private readonly AsyncState _state;

    public AsyncCompletionStep(AsyncState state)
    {
        _state = state;
    }

    public override unsafe void Run(bool canceled)
    {
        try
        {
            ((delegate* unmanaged<nint, void>)_state.CompletionRoutine)(_state.Block);
        }
        finally
        {
            _state.Done.Set();
        }
    }
}
