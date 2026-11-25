// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.Gdk;

/// <summary>
/// XPersistentLocalStoragePromptUserForSpaceAsync: there is no space-management UI, so the call succeeds when the
/// space is already free and ends as canceled (E_ABORT) when it is not.
/// </summary>
internal sealed class LocalStorageSpaceOperation : ManagedAsyncOperation
{
    private readonly SystemService _system;
    private readonly ulong _requested;

    public LocalStorageSpaceOperation(SystemService system, ulong requested)
    {
        _system = system;
        _requested = requested;
    }

    public override AsyncWorkResult Work()
    {
        int result = HResult.Abort;

        if (_system.LocalStorageFits(_requested))
        {
            result = HResult.Ok;
        }

        return new AsyncWorkResult(result, 0);
    }
}
