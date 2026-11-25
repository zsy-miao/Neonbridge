// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.Gdk;

/// <summary>XPackage.h entry points; the work is in <see cref="PackageService"/>.</summary>
internal static unsafe partial class XPackageShim
{
    [Shim(GdkModule.Name, "XPackageIsPackagedProcess")]
    [UnmanagedCallersOnly]
    private static byte XPackageIsPackagedProcess()
    {
        return GdkModule.True;
    }

    /// <summary>HRESULT XPackageGetCurrentProcessPackageIdentifier(size_t bufferSize, char* buffer).</summary>
    [Shim(GdkModule.Name, "XPackageGetCurrentProcessPackageIdentifier")]
    [UnmanagedCallersOnly]
    private static int XPackageGetCurrentProcessPackageIdentifier(nuint bufferSize, byte* buffer)
    {
        try
        {
            var gdk = ShimContext.Runtime.Gdk;

            try
            {
                return gdk.Packages.GetCurrentProcessPackageIdentifier(bufferSize, buffer);
            }
            catch (Exception e)
            {
                return gdk.Boundary.Failed(nameof(XPackageGetCurrentProcessPackageIdentifier), e);
            }
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XPackageShim.XPackageGetCurrentProcessPackageIdentifier");

            return HResult.Fail;
        }
    }

    /// <summary>HRESULT XPackageGetUserLocale(size_t localeSize, char* locale).</summary>
    [Shim(GdkModule.Name, "XPackageGetUserLocale")]
    [UnmanagedCallersOnly]
    private static int XPackageGetUserLocale(nuint size, byte* locale)
    {
        try
        {
            return PackageService.GetUserLocale(size, locale);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XPackageShim.XPackageGetUserLocale");

            return HResult.Fail;
        }
    }

    /// <summary>HRESULT XPackageEnumeratePackages(XPackageKind, XPackageEnumerationScope, void* context, XPackageEnumerationCallback*).</summary>
    [Shim(GdkModule.Name, "XPackageEnumeratePackages")]
    [UnmanagedCallersOnly]
    private static int XPackageEnumeratePackages(uint kind, uint scope, nint context, nint callback)
    {
        try
        {
            var gdk = ShimContext.Runtime.Gdk;

            try
            {
                return gdk.Packages.EnumeratePackages(kind, context, callback);
            }
            catch (Exception e)
            {
                return gdk.Boundary.Failed(nameof(XPackageEnumeratePackages), e);
            }
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XPackageShim.XPackageEnumeratePackages");

            return HResult.Fail;
        }
    }

    /// <summary>HRESULT XPackageMount(const char* packageIdentifier, XPackageMountHandle* mount) (deprecated, still exported).</summary>
    [Shim(GdkModule.Name, "XPackageMount")]
    [UnmanagedCallersOnly]
    private static int XPackageMount(byte* id, nint* mount)
    {
        try
        {
            var gdk = ShimContext.Runtime.Gdk;

            if (mount == null)
            {
                return HResult.Pointer;
            }

            try
            {
                int hr = gdk.Packages.Mount(Utf8Text.Read((nint)id), out nint handle);
                *mount = handle;

                return hr;
            }
            catch (Exception e)
            {
                return gdk.Boundary.Failed(nameof(XPackageMount), e);
            }
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XPackageShim.XPackageMount");

            return HResult.Fail;
        }
    }

    /// <summary>HRESULT XPackageMountWithUiAsync(const char* packageIdentifier, XAsyncBlock* async).</summary>
    [Shim(GdkModule.Name, "XPackageMountWithUiAsync")]
    [UnmanagedCallersOnly]
    private static int XPackageMountWithUiAsync(byte* id, nint async)
    {
        try
        {
            var gdk = ShimContext.Runtime.Gdk;

            try
            {
                return gdk.Async.BeginManaged(async, PackageService.MountApi, new MountOperation(gdk.Packages, Utf8Text.Read((nint)id)));
            }
            catch (Exception e)
            {
                return gdk.Boundary.Failed(nameof(XPackageMountWithUiAsync), e);
            }
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XPackageShim.XPackageMountWithUiAsync");

            return HResult.Fail;
        }
    }

    /// <summary>HRESULT XPackageMountWithUiResult(XAsyncBlock* async, XPackageMountHandle* mount).</summary>
    [Shim(GdkModule.Name, "XPackageMountWithUiResult")]
    [UnmanagedCallersOnly]
    private static int XPackageMountWithUiResult(nint async, nint* mount)
    {
        try
        {
            var gdk = ShimContext.Runtime.Gdk;

            if (mount == null)
            {
                return HResult.Pointer;
            }

            try
            {
                return gdk.Async.ManagedResult(async, PackageService.MountApi, (nuint)sizeof(nint), (nint)mount);
            }
            catch (Exception e)
            {
                return gdk.Boundary.Failed(nameof(XPackageMountWithUiResult), e);
            }
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XPackageShim.XPackageMountWithUiResult");

            return HResult.Fail;
        }
    }

    [Shim(GdkModule.Name, "XPackageGetMountPathSize")]
    [UnmanagedCallersOnly]
    private static int XPackageGetMountPathSize(nint mount, nuint* size)
    {
        try
        {
            return ShimContext.Runtime.Gdk.Packages.GetMountPathSize(mount, size);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XPackageShim.XPackageGetMountPathSize");

            return HResult.Fail;
        }
    }

    [Shim(GdkModule.Name, "XPackageGetMountPath")]
    [UnmanagedCallersOnly]
    private static int XPackageGetMountPath(nint mount, nuint size, byte* path)
    {
        try
        {
            return ShimContext.Runtime.Gdk.Packages.GetMountPath(mount, size, path);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XPackageShim.XPackageGetMountPath");

            return HResult.Fail;
        }
    }

    [Shim(GdkModule.Name, "XPackageCloseMountHandle")]
    [UnmanagedCallersOnly]
    private static void XPackageCloseMountHandle(nint mount)
    {
        try
        {
            ShimContext.Runtime.Gdk.Packages.CloseMount(mount);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XPackageShim.XPackageCloseMountHandle");
        }
    }

    /// <summary>HRESULT XPackageFindChunkAvailability(const char* packageIdentifier, uint32_t selectorCount, XPackageChunkSelector*, XPackageChunkAvailability*).</summary>
    [Shim(GdkModule.Name, "XPackageFindChunkAvailability")]
    [UnmanagedCallersOnly]
    private static int XPackageFindChunkAvailability(byte* id, uint selectorCount, nint selectors, uint* availability)
    {
        try
        {
            return ShimContext.Runtime.Gdk.Packages.FindChunkAvailability(id, availability);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XPackageShim.XPackageFindChunkAvailability");

            return HResult.Fail;
        }
    }

    /// <summary>HRESULT XPackageCreateInstallationMonitor(const char* id, uint32_t selectorCount, XPackageChunkSelector*, uint32_t minimumUpdateIntervalMs, XTaskQueueHandle, XPackageInstallationMonitorHandle*).</summary>
    [Shim(GdkModule.Name, "XPackageCreateInstallationMonitor")]
    [UnmanagedCallersOnly]
    private static int XPackageCreateInstallationMonitor(byte* id, uint selectorCount, nint selectors, uint interval, nint queue, nint* monitor)
    {
        try
        {
            var gdk = ShimContext.Runtime.Gdk;

            try
            {
                return gdk.Packages.CreateInstallationMonitor(id, queue, monitor);
            }
            catch (Exception e)
            {
                return gdk.Boundary.Failed(nameof(XPackageCreateInstallationMonitor), e);
            }
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XPackageShim.XPackageCreateInstallationMonitor");

            return HResult.Fail;
        }
    }

    /// <summary>void XPackageGetInstallationProgress(XPackageInstallationMonitorHandle, XPackageInstallationProgress*).</summary>
    [Shim(GdkModule.Name, "XPackageGetInstallationProgress")]
    [UnmanagedCallersOnly]
    private static void XPackageGetInstallationProgress(nint monitor, byte* progress)
    {
        try
        {
            ShimContext.Runtime.Gdk.Packages.GetInstallationProgress(monitor, progress);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XPackageShim.XPackageGetInstallationProgress");
        }
    }

    [Shim(GdkModule.Name, "XPackageUpdateInstallationMonitor")]
    [UnmanagedCallersOnly]
    private static void XPackageUpdateInstallationMonitor(nint monitor)
    {
    }

    [Shim(GdkModule.Name, "XPackageCloseInstallationMonitorHandle")]
    [UnmanagedCallersOnly]
    private static void XPackageCloseInstallationMonitorHandle(nint monitor)
    {
        try
        {
            ShimContext.Runtime.Gdk.Packages.CloseMonitor(monitor);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XPackageShim.XPackageCloseInstallationMonitorHandle");
        }
    }

    /// <summary>HRESULT XPackageRegisterInstallationProgressChanged(monitor, void* context, XPackageInstallationProgressCallback*, XTaskQueueRegistrationToken*).</summary>
    [Shim(GdkModule.Name, "XPackageRegisterInstallationProgressChanged")]
    [UnmanagedCallersOnly]
    private static int XPackageRegisterInstallationProgressChanged(nint monitor, nint context, nint callback, ulong* token)
    {
        try
        {
            var runtime = ShimContext.Runtime;

            try
            {
                int hr = runtime.Gdk.Packages.RegisterProgressChanged(monitor, context, callback, token);
                runtime.Log.Info($"  [gdk] XPackageRegisterInstallationProgressChanged(monitor 0x{monitor:x}, callback {runtime.Images.Describe(callback)}, context 0x{context:x}) -> 0x{hr:x8}, called back once (complete)");

                return hr;
            }
            catch (Exception e)
            {
                return runtime.Gdk.Boundary.Failed(nameof(XPackageRegisterInstallationProgressChanged), e);
            }
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XPackageShim.XPackageRegisterInstallationProgressChanged");

            return HResult.Fail;
        }
    }

    /// <summary>bool XPackageUnregisterInstallationProgressChanged(monitor, XTaskQueueRegistrationToken token, bool wait).</summary>
    [Shim(GdkModule.Name, "XPackageUnregisterInstallationProgressChanged")]
    [UnmanagedCallersOnly]
    private static byte XPackageUnregisterInstallationProgressChanged(nint monitor, ulong token, byte wait)
    {
        return GdkModule.True;
    }

    /// <summary>HRESULT XPackageEnumerateFeatures(const char* packageIdentifier, void* context, XPackageFeatureEnumerationCallback*).</summary>
    [Shim(GdkModule.Name, "XPackageEnumerateFeatures")]
    [UnmanagedCallersOnly]
    private static int XPackageEnumerateFeatures(byte* id, nint context, nint callback)
    {
        try
        {
            return ShimContext.Runtime.Gdk.Packages.EnumerateFeatures(id, callback);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XPackageShim.XPackageEnumerateFeatures");

            return HResult.Fail;
        }
    }

    /// <summary>HRESULT XPackageEstimateDownloadSize(const char* id, uint32_t selectorCount, XPackageChunkSelector*, uint64_t* downloadSize, bool* shouldPresentUserConfirmation).</summary>
    [Shim(GdkModule.Name, "XPackageEstimateDownloadSize")]
    [UnmanagedCallersOnly]
    private static int XPackageEstimateDownloadSize(byte* id, uint selectorCount, nint selectors, ulong* size, byte* confirm)
    {
        try
        {
            return ShimContext.Runtime.Gdk.Packages.EstimateDownloadSize(id, size, confirm);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XPackageShim.XPackageEstimateDownloadSize");

            return HResult.Fail;
        }
    }

    /// <summary>HRESULT XPackageInstallChunks(const char* id, uint32_t selectorCount, XPackageChunkSelector*, uint32_t minimumUpdateIntervalMs, bool suppressUserConfirmation, XTaskQueueHandle, XPackageInstallationMonitorHandle*).</summary>
    [Shim(GdkModule.Name, "XPackageInstallChunks")]
    [UnmanagedCallersOnly]
    private static int XPackageInstallChunks(byte* id, uint selectorCount, nint selectors, uint interval, byte suppressConfirmation, nint queue, nint* monitor)
    {
        try
        {
            var gdk = ShimContext.Runtime.Gdk;

            try
            {
                return gdk.Packages.CreateInstallationMonitor(id, queue, monitor);
            }
            catch (Exception e)
            {
                return gdk.Boundary.Failed(nameof(XPackageInstallChunks), e);
            }
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XPackageShim.XPackageInstallChunks");

            return HResult.Fail;
        }
    }

    /// <summary>HRESULT XPackageInstallChunksAsync(const char* id, uint32_t selectorCount, XPackageChunkSelector*, uint32_t minimumUpdateIntervalMs, bool suppressUserConfirmation, XAsyncBlock*).</summary>
    [Shim(GdkModule.Name, nameof(XPackageInstallChunksAsync))]
    [UnmanagedCallersOnly]
    private static int XPackageInstallChunksAsync(byte* id, uint selectorCount, nint selectors, uint interval, byte suppressConfirmation, nint async)
    {
        try
        {
            var gdk = ShimContext.Runtime.Gdk;

            try
            {
                nint queue = async == 0 ? 0 : *(nint*)(async + AsyncLayout.BlockQueueOffset);
                var operation = new InstallChunksOperation(gdk.Packages, Utf8Text.Read((nint)id), queue);

                return gdk.Async.BeginManaged(async, nameof(XPackageInstallChunksAsync), operation);
            }
            catch (Exception e)
            {
                return gdk.Boundary.Failed(nameof(XPackageInstallChunksAsync), e);
            }
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XPackageShim.XPackageInstallChunksAsync");

            return HResult.Fail;
        }
    }

    /// <summary>HRESULT XPackageInstallChunksResult(XAsyncBlock*, XPackageInstallationMonitorHandle*).</summary>
    [Shim(GdkModule.Name, "XPackageInstallChunksResult")]
    [UnmanagedCallersOnly]
    private static int XPackageInstallChunksResult(nint async, nint* monitor)
    {
        try
        {
            var gdk = ShimContext.Runtime.Gdk;

            if (monitor == null)
            {
                return HResult.Pointer;
            }

            try
            {
                return gdk.Async.ManagedResult(async, nameof(XPackageInstallChunksAsync), (nuint)sizeof(nint), (nint)monitor);
            }
            catch (Exception e)
            {
                return gdk.Boundary.Failed(nameof(XPackageInstallChunksResult), e);
            }
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XPackageShim.XPackageInstallChunksResult");

            return HResult.Fail;
        }
    }

    /// <summary>HRESULT XPackageChangeChunkInstallOrder(const char* id, uint32_t selectorCount, XPackageChunkSelector*).</summary>
    [Shim(GdkModule.Name, "XPackageChangeChunkInstallOrder")]
    [UnmanagedCallersOnly]
    private static int XPackageChangeChunkInstallOrder(byte* id, uint selectorCount, nint selectors)
    {
        try
        {
            return ShimContext.Runtime.Gdk.Packages.ChangeChunkInstallOrder(id);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XPackageShim.XPackageChangeChunkInstallOrder");

            return HResult.Fail;
        }
    }

    /// <summary>HRESULT XPackageUninstallChunks(const char* id, uint32_t selectorCount, XPackageChunkSelector*): the loader never deletes game files.</summary>
    [Shim(GdkModule.Name, "XPackageUninstallChunks")]
    [UnmanagedCallersOnly]
    private static int XPackageUninstallChunks(byte* id, uint selectorCount, nint selectors)
    {
        try
        {
            ShimContext.Runtime.Gdk.Boundary.Info($"XPackageUninstallChunks({Utf8Text.Read((nint)id)}) not supported");

            return HResult.NotSupported;
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XPackageShim.XPackageUninstallChunks");

            return HResult.Fail;
        }
    }

    /// <summary>bool XPackageUninstallPackage(const char* packageIdentifier): false, nothing is uninstalled.</summary>
    [Shim(GdkModule.Name, "XPackageUninstallPackage")]
    [UnmanagedCallersOnly]
    private static byte XPackageUninstallPackage(byte* id)
    {
        try
        {
            ShimContext.Runtime.Gdk.Boundary.Info($"XPackageUninstallPackage({Utf8Text.Read((nint)id)}) not supported");

            return GdkModule.False;
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XPackageShim.XPackageUninstallPackage");

            return default;
        }
    }

    [Shim(GdkModule.Name, "XPackageGetWriteStats")]
    [UnmanagedCallersOnly]
    private static int XPackageGetWriteStats(byte* stats)
    {
        try
        {
            return PackageService.GetWriteStats(stats);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XPackageShim.XPackageGetWriteStats");

            return HResult.Fail;
        }
    }

    /// <summary>HRESULT XPackageRegisterPackageInstalled(XTaskQueueHandle, void* context, XPackageInstalledCallback*, XTaskQueueRegistrationToken*): no DLC ever installs.</summary>
    [Shim(GdkModule.Name, "XPackageRegisterPackageInstalled")]
    [UnmanagedCallersOnly]
    private static int XPackageRegisterPackageInstalled(nint queue, nint context, nint callback, ulong* token)
    {
        try
        {
            return ShimContext.Runtime.Gdk.Registrations.Register("XPackageRegisterPackageInstalled", callback, token);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XPackageShim.XPackageRegisterPackageInstalled");

            return HResult.Fail;
        }
    }

    [Shim(GdkModule.Name, "XPackageUnregisterPackageInstalled")]
    [UnmanagedCallersOnly]
    private static byte XPackageUnregisterPackageInstalled(ulong token, byte wait)
    {
        try
        {
            return ShimContext.Runtime.Gdk.Registrations.Unregister(token);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XPackageShim.XPackageUnregisterPackageInstalled");

            return default;
        }
    }
}
