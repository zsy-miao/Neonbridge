// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Text;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.Gdk;

/// <summary>
/// XStore.h offline: store contexts, the game license and an unreachable store. Calls that need the store service
/// (add-on licenses, consumables, user ids, downloads) fail with E_GAMESTORE_NETWORK_ERROR; product and entitlement
/// queries return an empty list; purchase UI ends as canceled. No license handle is ever issued.
/// </summary>
internal sealed unsafe class StoreService
{
    public const string GameLicenseApi = "XStoreQueryGameLicenseAsync";

    /// <summary>E_GAMESTORE_NETWORK_ERROR (XGameErr.h): the store service could not be reached.</summary>
    public const int NetworkError = unchecked((int)0x89245301);

    /// <summary>
    /// struct XStoreGameLicense { char skuStoreId[18]; bool isActive; bool isTrialOwnedByThisUser; bool isDiscLicense;
    /// bool isTrial; uint32_t trialTimeRemainingInSeconds; char trialUniqueId[64]; time_t expirationDate; }: 104 bytes.
    /// </summary>
    public const int GameLicenseSize = 104;

    public const int IsActiveOffset = 18;
    public const int IsTrialOffset = 21;
    private const int SkuCapacity = 17;
    private const int ExpirationOffset = 96;
    private const string SkuSuffix = "/0010";

    private readonly HandleTable<object> _contexts = new("XSTORCTX");
    private readonly HandleTable<object> _productQueries = new("XSTORPQH");
    private readonly GdkConfig _config;
    private readonly PackageService _packages;

    public StoreService(GdkConfig config, PackageService packages)
    {
        _config = config;
        _packages = packages;
    }

    public int OpenProductQueries => _productQueries.Count;

    public int CreateContext(nint* context)
    {
        if (context == null)
        {
            return HResult.Pointer;
        }

        *context = _contexts.Add(new object());

        return HResult.Ok;
    }

    public void CloseContext(nint context)
    {
        _contexts.Remove(context);
    }

    public bool IsContext(nint context)
    {
        return _contexts.Get(context) is not null;
    }

    /// <summary>Writes the license: the store id with "/0010" as SKU, active, never expiring.</summary>
    public void WriteGameLicense(byte* license)
    {
        new Span<byte>(license, GameLicenseSize).Clear();
        string storeId = _config.Identity.StoreId;
        string sku = (storeId + SkuSuffix)[..Math.Min(SkuCapacity, storeId.Length + SkuSuffix.Length)];
        var bytes = Encoding.ASCII.GetBytes(sku);
        bytes.CopyTo(new Span<byte>(license, Math.Min(SkuCapacity, bytes.Length)));

        license[IsActiveOffset] = GdkModule.True;
        *(long*)(license + ExpirationOffset) = long.MaxValue;
    }

    /// <summary>A products query handle with no products and no further pages.</summary>
    public nint CreateProductQuery()
    {
        return _productQueries.Add(new object());
    }

    public bool IsProductQuery(nint query)
    {
        return _productQueries.Get(query) is not null;
    }

    public void CloseProductQuery(nint query)
    {
        _productQueries.Remove(query);
    }

    /// <summary>
    /// XStoreQueryPackageIdentifier: the game's own store id maps to the game package; no other product (DLC) is
    /// installed, so any other id is E_NOTFOUND.
    /// </summary>
    public int QueryPackageIdentifier(string? storeId, nuint size, byte* packageIdentifier)
    {
        if (string.IsNullOrEmpty(storeId) || packageIdentifier == null)
        {
            return HResult.InvalidArgument;
        }

        if (!string.Equals(storeId, _config.Identity.StoreId, StringComparison.OrdinalIgnoreCase))
        {
            return HResult.NotFound;
        }

        return Utf8Text.Write(_packages.PackageId, size, packageIdentifier, null);
    }
}
