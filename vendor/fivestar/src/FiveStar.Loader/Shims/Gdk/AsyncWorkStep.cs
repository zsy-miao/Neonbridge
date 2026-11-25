// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.Gdk;

/// <summary>Runs a provider's DoWork on the work port.</summary>
internal sealed class AsyncWorkStep : AsyncStep
{
    private readonly AsyncService _service;
    private readonly AsyncState _state;

    public AsyncWorkStep(AsyncService service, AsyncState state)
    {
        _service = service;
        _state = state;
    }

    public override void Run(bool canceled)
    {
        _service.RunWork(_state, canceled);
    }
}
