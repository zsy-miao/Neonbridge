// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.Gdk;

/// <summary>An XStore products query: the result is an XStoreProductQueryHandle of an empty, single page.</summary>
internal sealed class ProductQueryOperation : ManagedAsyncOperation
{
    private readonly StoreService _store;

    public ProductQueryOperation(StoreService store)
    {
        _store = store;
    }

    public override unsafe AsyncWorkResult Work()
    {
        return new AsyncWorkResult(HResult.Ok, (nuint)sizeof(nint));
    }

    public override unsafe int Write(byte* buffer, nuint size)
    {
        *(nint*)buffer = _store.CreateProductQuery();

        return HResult.Ok;
    }
}
