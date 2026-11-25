// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.Gdk;

/// <summary>
/// An *Async call the offline runtime answers at once with a fixed HRESULT and no result: a store, token or URL query
/// that needs the network (an error), or one whose answer is an empty list (S_OK).
/// </summary>
internal sealed class OfflineOperation : ManagedAsyncOperation
{
    private readonly int _result;

    public OfflineOperation(int result)
    {
        _result = result;
    }

    public override AsyncWorkResult Work()
    {
        return new AsyncWorkResult(_result, 0);
    }
}
