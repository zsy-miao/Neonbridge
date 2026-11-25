// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.Gdk;

/// <summary>XStoreQueryGameLicenseAsync: the result is an XStoreGameLicense.</summary>
internal sealed class GameLicenseOperation : ManagedAsyncOperation
{
    private readonly StoreService _store;

    public GameLicenseOperation(StoreService store)
    {
        _store = store;
    }

    public override AsyncWorkResult Work()
    {
        return new AsyncWorkResult(HResult.Ok, StoreService.GameLicenseSize);
    }

    public override unsafe int Write(byte* buffer, nuint size)
    {
        _store.WriteGameLicense(buffer);

        return HResult.Ok;
    }
}
