// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.Gdk;

/// <summary>
/// XGameSave.h and XGameSaveFiles.h entry points; the work is in <see cref="GameSaveService"/>. Callbacks are
/// <c>bool XGameSaveContainerInfoCallback(const XGameSaveContainerInfo*, void* context)</c> and
/// <c>bool XGameSaveBlobInfoCallback(const XGameSaveBlobInfo*, void* context)</c>.
/// </summary>
internal static unsafe partial class XGameSaveShim
{
    /// <summary>HRESULT XGameSaveInitializeProvider(XUserHandle, const char* configurationId, bool syncOnDemand, XGameSaveProviderHandle*).</summary>
    [Shim(GdkModule.Name, "XGameSaveInitializeProvider")]
    [UnmanagedCallersOnly]
    private static int XGameSaveInitializeProvider(nint user, byte* configId, byte syncOnDemand, nint* provider)
    {
        try
        {
            var gdk = ShimContext.Runtime.Gdk;

            if (provider == null)
            {
                return HResult.Pointer;
            }

            try
            {
                int hr = gdk.Saves.InitializeProvider(user, Utf8Text.Read((nint)configId), out nint created);
                *provider = created;

                return hr;
            }
            catch (Exception e)
            {
                return gdk.Boundary.Failed(nameof(XGameSaveInitializeProvider), e);
            }
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XGameSaveShim.XGameSaveInitializeProvider");

            return HResult.Fail;
        }
    }

    /// <summary>HRESULT XGameSaveInitializeProviderAsync(XUserHandle, const char* configurationId, bool syncOnDemand, XAsyncBlock*).</summary>
    [Shim(GdkModule.Name, "XGameSaveInitializeProviderAsync")]
    [UnmanagedCallersOnly]
    private static int XGameSaveInitializeProviderAsync(nint user, byte* configId, byte syncOnDemand, nint async)
    {
        try
        {
            var gdk = ShimContext.Runtime.Gdk;

            try
            {
                return gdk.Saves.InitializeProviderAsync(user, Utf8Text.Read((nint)configId), async);
            }
            catch (Exception e)
            {
                return gdk.Boundary.Failed(nameof(XGameSaveInitializeProviderAsync), e);
            }
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XGameSaveShim.XGameSaveInitializeProviderAsync");

            return HResult.Fail;
        }
    }

    /// <summary>HRESULT XGameSaveInitializeProviderResult(XAsyncBlock*, XGameSaveProviderHandle*).</summary>
    [Shim(GdkModule.Name, "XGameSaveInitializeProviderResult")]
    [UnmanagedCallersOnly]
    private static int XGameSaveInitializeProviderResult(nint async, nint* provider)
    {
        try
        {
            return ManagedResult(nameof(XGameSaveInitializeProviderResult), async, GameSaveService.InitializeProviderApi, (nuint)sizeof(nint), provider);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XGameSaveShim.XGameSaveInitializeProviderResult");

            return HResult.Fail;
        }
    }

    [Shim(GdkModule.Name, "XGameSaveCloseProvider")]
    [UnmanagedCallersOnly]
    private static void XGameSaveCloseProvider(nint provider)
    {
        try
        {
            ShimContext.Runtime.Gdk.Saves.CloseProvider(provider);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XGameSaveShim.XGameSaveCloseProvider");
        }
    }

    /// <summary>HRESULT XGameSaveGetRemainingQuota(XGameSaveProviderHandle, int64_t* remainingQuota).</summary>
    [Shim(GdkModule.Name, "XGameSaveGetRemainingQuota")]
    [UnmanagedCallersOnly]
    private static int XGameSaveGetRemainingQuota(nint provider, long* remaining)
    {
        try
        {
            var gdk = ShimContext.Runtime.Gdk;

            try
            {
                return gdk.Saves.GetRemainingQuota(provider, remaining);
            }
            catch (Exception e)
            {
                return gdk.Boundary.Failed(nameof(XGameSaveGetRemainingQuota), e);
            }
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XGameSaveShim.XGameSaveGetRemainingQuota");

            return HResult.Fail;
        }
    }

    [Shim(GdkModule.Name, "XGameSaveGetRemainingQuotaAsync")]
    [UnmanagedCallersOnly]
    private static int XGameSaveGetRemainingQuotaAsync(nint provider, nint async)
    {
        try
        {
            var gdk = ShimContext.Runtime.Gdk;

            try
            {
                return gdk.Saves.GetRemainingQuotaAsync(provider, async);
            }
            catch (Exception e)
            {
                return gdk.Boundary.Failed(nameof(XGameSaveGetRemainingQuotaAsync), e);
            }
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XGameSaveShim.XGameSaveGetRemainingQuotaAsync");

            return HResult.Fail;
        }
    }

    [Shim(GdkModule.Name, "XGameSaveGetRemainingQuotaResult")]
    [UnmanagedCallersOnly]
    private static int XGameSaveGetRemainingQuotaResult(nint async, long* remaining)
    {
        try
        {
            return ManagedResult(nameof(XGameSaveGetRemainingQuotaResult), async, GameSaveService.RemainingQuotaApi, sizeof(long), remaining);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XGameSaveShim.XGameSaveGetRemainingQuotaResult");

            return HResult.Fail;
        }
    }

    /// <summary>HRESULT XGameSaveDeleteContainer(XGameSaveProviderHandle, const char* containerName).</summary>
    [Shim(GdkModule.Name, "XGameSaveDeleteContainer")]
    [UnmanagedCallersOnly]
    private static int XGameSaveDeleteContainer(nint provider, byte* name)
    {
        try
        {
            var gdk = ShimContext.Runtime.Gdk;

            try
            {
                return gdk.Saves.DeleteContainer(provider, name);
            }
            catch (Exception e)
            {
                return gdk.Boundary.Failed(nameof(XGameSaveDeleteContainer), e);
            }
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XGameSaveShim.XGameSaveDeleteContainer");

            return HResult.Fail;
        }
    }

    [Shim(GdkModule.Name, "XGameSaveDeleteContainerAsync")]
    [UnmanagedCallersOnly]
    private static int XGameSaveDeleteContainerAsync(nint provider, byte* name, nint async)
    {
        try
        {
            var gdk = ShimContext.Runtime.Gdk;

            try
            {
                return gdk.Saves.DeleteContainerAsync(provider, name, async);
            }
            catch (Exception e)
            {
                return gdk.Boundary.Failed(nameof(XGameSaveDeleteContainerAsync), e);
            }
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XGameSaveShim.XGameSaveDeleteContainerAsync");

            return HResult.Fail;
        }
    }

    [Shim(GdkModule.Name, "XGameSaveDeleteContainerResult")]
    [UnmanagedCallersOnly]
    private static int XGameSaveDeleteContainerResult(nint async)
    {
        try
        {
            return ManagedResult(nameof(XGameSaveDeleteContainerResult), async, GameSaveService.DeleteContainerApi, 0, (void*)null);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XGameSaveShim.XGameSaveDeleteContainerResult");

            return HResult.Fail;
        }
    }

    /// <summary>HRESULT XGameSaveCreateContainer(XGameSaveProviderHandle, const char* containerName, XGameSaveContainerHandle*).</summary>
    [Shim(GdkModule.Name, "XGameSaveCreateContainer")]
    [UnmanagedCallersOnly]
    private static int XGameSaveCreateContainer(nint provider, byte* name, nint* container)
    {
        try
        {
            var gdk = ShimContext.Runtime.Gdk;

            try
            {
                return gdk.Saves.CreateContainer(provider, name, container);
            }
            catch (Exception e)
            {
                return gdk.Boundary.Failed(nameof(XGameSaveCreateContainer), e);
            }
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XGameSaveShim.XGameSaveCreateContainer");

            return HResult.Fail;
        }
    }

    [Shim(GdkModule.Name, "XGameSaveCloseContainer")]
    [UnmanagedCallersOnly]
    private static void XGameSaveCloseContainer(nint container)
    {
        try
        {
            ShimContext.Runtime.Gdk.Saves.CloseContainer(container);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XGameSaveShim.XGameSaveCloseContainer");
        }
    }

    /// <summary>HRESULT XGameSaveGetContainerInfo(XGameSaveProviderHandle, const char* containerName, void* context, XGameSaveContainerInfoCallback*).</summary>
    [Shim(GdkModule.Name, "XGameSaveGetContainerInfo")]
    [UnmanagedCallersOnly]
    private static int XGameSaveGetContainerInfo(nint provider, byte* name, nint context, nint callback)
    {
        try
        {
            var gdk = ShimContext.Runtime.Gdk;

            try
            {
                return gdk.Saves.GetContainerInfo(provider, name, context, callback);
            }
            catch (Exception e)
            {
                return gdk.Boundary.Failed(nameof(XGameSaveGetContainerInfo), e);
            }
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XGameSaveShim.XGameSaveGetContainerInfo");

            return HResult.Fail;
        }
    }

    [Shim(GdkModule.Name, "XGameSaveEnumerateContainerInfo")]
    [UnmanagedCallersOnly]
    private static int XGameSaveEnumerateContainerInfo(nint provider, nint context, nint callback)
    {
        try
        {
            var gdk = ShimContext.Runtime.Gdk;

            try
            {
                return gdk.Saves.EnumerateContainers(provider, null, context, callback);
            }
            catch (Exception e)
            {
                return gdk.Boundary.Failed(nameof(XGameSaveEnumerateContainerInfo), e);
            }
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XGameSaveShim.XGameSaveEnumerateContainerInfo");

            return HResult.Fail;
        }
    }

    [Shim(GdkModule.Name, "XGameSaveEnumerateContainerInfoByName")]
    [UnmanagedCallersOnly]
    private static int XGameSaveEnumerateContainerInfoByName(nint provider, byte* prefix, nint context, nint callback)
    {
        try
        {
            var gdk = ShimContext.Runtime.Gdk;

            try
            {
                return gdk.Saves.EnumerateContainers(provider, Utf8Text.Read((nint)prefix), context, callback);
            }
            catch (Exception e)
            {
                return gdk.Boundary.Failed(nameof(XGameSaveEnumerateContainerInfoByName), e);
            }
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XGameSaveShim.XGameSaveEnumerateContainerInfoByName");

            return HResult.Fail;
        }
    }

    [Shim(GdkModule.Name, "XGameSaveEnumerateBlobInfo")]
    [UnmanagedCallersOnly]
    private static int XGameSaveEnumerateBlobInfo(nint container, nint context, nint callback)
    {
        try
        {
            var gdk = ShimContext.Runtime.Gdk;

            try
            {
                return gdk.Saves.EnumerateBlobs(container, null, context, callback);
            }
            catch (Exception e)
            {
                return gdk.Boundary.Failed(nameof(XGameSaveEnumerateBlobInfo), e);
            }
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XGameSaveShim.XGameSaveEnumerateBlobInfo");

            return HResult.Fail;
        }
    }

    [Shim(GdkModule.Name, "XGameSaveEnumerateBlobInfoByName")]
    [UnmanagedCallersOnly]
    private static int XGameSaveEnumerateBlobInfoByName(nint container, byte* prefix, nint context, nint callback)
    {
        try
        {
            var gdk = ShimContext.Runtime.Gdk;

            try
            {
                return gdk.Saves.EnumerateBlobs(container, Utf8Text.Read((nint)prefix), context, callback);
            }
            catch (Exception e)
            {
                return gdk.Boundary.Failed(nameof(XGameSaveEnumerateBlobInfoByName), e);
            }
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XGameSaveShim.XGameSaveEnumerateBlobInfoByName");

            return HResult.Fail;
        }
    }

    /// <summary>HRESULT XGameSaveReadBlobData(XGameSaveContainerHandle, const char** blobNames, uint32_t* countOfBlobs, size_t blobsSize, XGameSaveBlob* blobData).</summary>
    [Shim(GdkModule.Name, "XGameSaveReadBlobData")]
    [UnmanagedCallersOnly]
    private static int XGameSaveReadBlobData(nint container, byte** blobNames, uint* countOfBlobs, nuint blobsSize, byte* blobData)
    {
        try
        {
            var gdk = ShimContext.Runtime.Gdk;

            try
            {
                return gdk.Saves.ReadBlobData(container, blobNames, countOfBlobs, blobsSize, blobData);
            }
            catch (Exception e)
            {
                return gdk.Boundary.Failed(nameof(XGameSaveReadBlobData), e);
            }
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XGameSaveShim.XGameSaveReadBlobData");

            return HResult.Fail;
        }
    }

    /// <summary>HRESULT XGameSaveReadBlobDataAsync(XGameSaveContainerHandle, const char** blobNames, uint32_t countOfBlobs, XAsyncBlock*).</summary>
    [Shim(GdkModule.Name, "XGameSaveReadBlobDataAsync")]
    [UnmanagedCallersOnly]
    private static int XGameSaveReadBlobDataAsync(nint container, byte** blobNames, uint countOfBlobs, nint async)
    {
        try
        {
            var gdk = ShimContext.Runtime.Gdk;

            try
            {
                return gdk.Saves.ReadBlobDataAsync(container, blobNames, countOfBlobs, async);
            }
            catch (Exception e)
            {
                return gdk.Boundary.Failed(nameof(XGameSaveReadBlobDataAsync), e);
            }
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XGameSaveShim.XGameSaveReadBlobDataAsync");

            return HResult.Fail;
        }
    }

    /// <summary>HRESULT XGameSaveReadBlobDataResult(XAsyncBlock*, size_t blobsSize, XGameSaveBlob* blobData, uint32_t* countOfBlobs).</summary>
    [Shim(GdkModule.Name, "XGameSaveReadBlobDataResult")]
    [UnmanagedCallersOnly]
    private static int XGameSaveReadBlobDataResult(nint async, nuint blobsSize, byte* blobData, uint* countOfBlobs)
    {
        try
        {
            var gdk = ShimContext.Runtime.Gdk;

            try
            {
                return gdk.Saves.ReadBlobDataResult(async, blobsSize, blobData, countOfBlobs);
            }
            catch (Exception e)
            {
                return gdk.Boundary.Failed(nameof(XGameSaveReadBlobDataResult), e);
            }
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XGameSaveShim.XGameSaveReadBlobDataResult");

            return HResult.Fail;
        }
    }

    /// <summary>HRESULT XGameSaveCreateUpdate(XGameSaveContainerHandle, const char* containerDisplayName, XGameSaveUpdateHandle*).</summary>
    [Shim(GdkModule.Name, "XGameSaveCreateUpdate")]
    [UnmanagedCallersOnly]
    private static int XGameSaveCreateUpdate(nint container, byte* displayName, nint* update)
    {
        try
        {
            var gdk = ShimContext.Runtime.Gdk;

            try
            {
                return gdk.Saves.CreateUpdate(container, displayName, update);
            }
            catch (Exception e)
            {
                return gdk.Boundary.Failed(nameof(XGameSaveCreateUpdate), e);
            }
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XGameSaveShim.XGameSaveCreateUpdate");

            return HResult.Fail;
        }
    }

    /// <summary>HRESULT XGameSaveSubmitBlobWrite(XGameSaveUpdateHandle, const char* blobName, const uint8_t* data, size_t byteCount).</summary>
    [Shim(GdkModule.Name, "XGameSaveSubmitBlobWrite")]
    [UnmanagedCallersOnly]
    private static int XGameSaveSubmitBlobWrite(nint update, byte* blobName, byte* data, nuint byteCount)
    {
        try
        {
            var gdk = ShimContext.Runtime.Gdk;

            try
            {
                return gdk.Saves.SubmitBlobWrite(update, blobName, data, byteCount);
            }
            catch (Exception e)
            {
                return gdk.Boundary.Failed(nameof(XGameSaveSubmitBlobWrite), e);
            }
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XGameSaveShim.XGameSaveSubmitBlobWrite");

            return HResult.Fail;
        }
    }

    [Shim(GdkModule.Name, "XGameSaveSubmitBlobDelete")]
    [UnmanagedCallersOnly]
    private static int XGameSaveSubmitBlobDelete(nint update, byte* blobName)
    {
        try
        {
            var gdk = ShimContext.Runtime.Gdk;

            try
            {
                return gdk.Saves.SubmitBlobDelete(update, blobName);
            }
            catch (Exception e)
            {
                return gdk.Boundary.Failed(nameof(XGameSaveSubmitBlobDelete), e);
            }
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XGameSaveShim.XGameSaveSubmitBlobDelete");

            return HResult.Fail;
        }
    }

    [Shim(GdkModule.Name, "XGameSaveSubmitUpdate")]
    [UnmanagedCallersOnly]
    private static int XGameSaveSubmitUpdate(nint update)
    {
        try
        {
            var gdk = ShimContext.Runtime.Gdk;

            try
            {
                return gdk.Saves.SubmitUpdate(update);
            }
            catch (Exception e)
            {
                return gdk.Boundary.Failed(nameof(XGameSaveSubmitUpdate), e);
            }
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XGameSaveShim.XGameSaveSubmitUpdate");

            return HResult.Fail;
        }
    }

    [Shim(GdkModule.Name, "XGameSaveSubmitUpdateAsync")]
    [UnmanagedCallersOnly]
    private static int XGameSaveSubmitUpdateAsync(nint update, nint async)
    {
        try
        {
            var gdk = ShimContext.Runtime.Gdk;

            try
            {
                return gdk.Saves.SubmitUpdateAsync(update, async);
            }
            catch (Exception e)
            {
                return gdk.Boundary.Failed(nameof(XGameSaveSubmitUpdateAsync), e);
            }
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XGameSaveShim.XGameSaveSubmitUpdateAsync");

            return HResult.Fail;
        }
    }

    [Shim(GdkModule.Name, "XGameSaveSubmitUpdateResult")]
    [UnmanagedCallersOnly]
    private static int XGameSaveSubmitUpdateResult(nint async)
    {
        try
        {
            return ManagedResult(nameof(XGameSaveSubmitUpdateResult), async, GameSaveService.SubmitUpdateApi, 0, (void*)null);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XGameSaveShim.XGameSaveSubmitUpdateResult");

            return HResult.Fail;
        }
    }

    [Shim(GdkModule.Name, "XGameSaveCloseUpdate")]
    [UnmanagedCallersOnly]
    private static void XGameSaveCloseUpdate(nint update)
    {
        try
        {
            ShimContext.Runtime.Gdk.Saves.CloseUpdate(update);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XGameSaveShim.XGameSaveCloseUpdate");
        }
    }

    /// <summary>HRESULT XGameSaveFilesGetFolderWithUiAsync(XUserHandle, const char* configurationId, XAsyncBlock*).</summary>
    [Shim(GdkModule.Name, "XGameSaveFilesGetFolderWithUiAsync")]
    [UnmanagedCallersOnly]
    private static int XGameSaveFilesGetFolderWithUiAsync(nint user, byte* configId, nint async)
    {
        try
        {
            var gdk = ShimContext.Runtime.Gdk;

            try
            {
                return gdk.Saves.GetFolderWithUiAsync(user, configId, async);
            }
            catch (Exception e)
            {
                return gdk.Boundary.Failed(nameof(XGameSaveFilesGetFolderWithUiAsync), e);
            }
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XGameSaveShim.XGameSaveFilesGetFolderWithUiAsync");

            return HResult.Fail;
        }
    }

    /// <summary>HRESULT XGameSaveFilesGetFolderWithUiResult(XAsyncBlock*, size_t folderSize, char* folderResult); callers pass MAX_PATH.</summary>
    [Shim(GdkModule.Name, "XGameSaveFilesGetFolderWithUiResult")]
    [UnmanagedCallersOnly]
    private static int XGameSaveFilesGetFolderWithUiResult(nint async, nuint folderSize, byte* folder)
    {
        try
        {
            var gdk = ShimContext.Runtime.Gdk;

            try
            {
                return gdk.Async.ManagedResult(async, GameSaveService.SaveFolderApi, folderSize, (nint)folder);
            }
            catch (Exception e)
            {
                return gdk.Boundary.Failed(nameof(XGameSaveFilesGetFolderWithUiResult), e);
            }
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XGameSaveShim.XGameSaveFilesGetFolderWithUiResult");

            return HResult.Fail;
        }
    }

    /// <summary>HRESULT XGameSaveFilesGetRemainingQuota(XUserHandle, const char* configurationId, int64_t* remainingQuota).</summary>
    [Shim(GdkModule.Name, "XGameSaveFilesGetRemainingQuota")]
    [UnmanagedCallersOnly]
    private static int XGameSaveFilesGetRemainingQuota(nint user, byte* configId, long* remaining)
    {
        try
        {
            var gdk = ShimContext.Runtime.Gdk;

            try
            {
                return gdk.Saves.GetSaveFilesRemainingQuota(user, configId, remaining);
            }
            catch (Exception e)
            {
                return gdk.Boundary.Failed(nameof(XGameSaveFilesGetRemainingQuota), e);
            }
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XGameSaveShim.XGameSaveFilesGetRemainingQuota");

            return HResult.Fail;
        }
    }

    /// <summary>The *Result half of an async call whose result is a fixed-size value; a null destination is E_POINTER when one is expected.</summary>
    private static int ManagedResult(string api, nint async, string asyncApi, nuint size, void* destination)
    {
        var gdk = ShimContext.Runtime.Gdk;

        if (size != 0 && destination == null)
        {
            return HResult.Pointer;
        }

        try
        {
            return gdk.Async.ManagedResult(async, asyncApi, size, (nint)destination);
        }
        catch (Exception e)
        {
            return gdk.Boundary.Failed(api, e);
        }
    }
}
