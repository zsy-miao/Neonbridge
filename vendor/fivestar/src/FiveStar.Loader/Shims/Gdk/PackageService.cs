// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.Gdk;

/// <summary>
/// XPackage.h: one installed, fully available game package rooted at the package root, with the identity from
/// MicrosoftGame.config. No DLC; chunks are always ready; installation monitors report 100% and tell each progress
/// callback so once.
/// </summary>
internal sealed unsafe class PackageService
{
    public const string MountApi = "XPackageMountWithUiAsync";

    private const string UserLocale = "en-US";

    private readonly HandleTable<string> _mounts = new("XPKMOUNT");
    private readonly HandleTable<InstallationMonitor> _monitors = new("XPKMONIT");
    private readonly GdkConfig _config;
    private readonly TaskQueueService _taskQueues;
    private long _token;

    public PackageService(GdkConfig config, TaskQueueService taskQueues)
    {
        _config = config;
        _taskQueues = taskQueues;
    }

    /// <summary>
    /// The package identifier: the identity name from MicrosoftGame.config (shorter than XPACKAGE_IDENTIFIER_MAX_LENGTH),
    /// else the title id. What the console returns here is unconfirmed.
    /// </summary>
    public string PackageId
    {
        get
        {
            var identity = _config.Identity;

            if (identity.Name.Length > 0)
            {
                return identity.Name;
            }

            return _config.TitleIdText;
        }
    }

    public int GetCurrentProcessPackageIdentifier(nuint size, byte* buffer)
    {
        return Utf8Text.Write(PackageId, size, buffer, null);
    }

    public static int GetUserLocale(nuint size, byte* locale)
    {
        return Utf8Text.Write(UserLocale, size, locale, null);
    }

    /// <summary>Calls the callback once for the game package; no content packages.</summary>
    public int EnumeratePackages(uint kind, nint context, nint callback)
    {
        if (callback == 0)
        {
            return HResult.InvalidArgument;
        }

        if (kind != PackageLayout.KindGame)
        {
            return HResult.Ok;
        }

        var identity = _config.Identity;
        string[] texts = [PackageId, identity.DisplayName, identity.Description, identity.PublisherDisplayName, identity.StoreId, _config.TitleIdText];
        var pointers = texts.Select(t => (nint)Utf8Text.Allocate(t)).ToArray();
        byte* details = stackalloc byte[PackageLayout.DetailsSize];
        WriteDetails(details, pointers, identity.Version);

        try
        {
            ((delegate* unmanaged<nint, nint, byte>)callback)(context, (nint)details);
        }
        finally
        {
            foreach (nint pointer in pointers)
            {
                NativeMemory.Free((void*)pointer);
            }
        }

        return HResult.Ok;
    }

    public int Mount(string? id, out nint mount)
    {
        mount = 0;

        if (!IsOurs(id))
        {
            return HResult.NotFound;
        }

        mount = _mounts.Add(_config.PackageRoot);

        return HResult.Ok;
    }

    public int GetMountPathSize(nint mount, nuint* size)
    {
        string? path = _mounts.Get(mount);

        if (path is null || size == null)
        {
            return HResult.InvalidArgument;
        }

        *size = Utf8Text.SizeWithTerminator(path);

        return HResult.Ok;
    }

    public int GetMountPath(nint mount, nuint size, byte* path)
    {
        string? root = _mounts.Get(mount);

        if (root is null)
        {
            return HResult.InvalidArgument;
        }

        return Utf8Text.Write(root, size, path, null);
    }

    public void CloseMount(nint mount)
    {
        _mounts.Remove(mount);
    }

    public int FindChunkAvailability(byte* id, uint* availability)
    {
        if (availability == null)
        {
            return HResult.Pointer;
        }

        if (!IsOurs(Utf8Text.Read((nint)id)))
        {
            return HResult.NotFound;
        }

        *availability = PackageLayout.ChunkReady;

        return HResult.Ok;
    }

    public int CreateInstallationMonitor(byte* id, nint queue, nint* monitor)
    {
        if (monitor == null)
        {
            return HResult.Pointer;
        }

        int hr = CreateMonitor(Utf8Text.Read((nint)id), queue, out nint created);
        *monitor = created;

        return hr;
    }

    /// <summary>
    /// An installation monitor of the game package, already complete, whose callbacks run on <paramref name="queue"/>;
    /// E_NOTFOUND for another package.
    /// </summary>
    public int CreateMonitor(string? id, nint queue, out nint monitor)
    {
        monitor = 0;

        if (!IsOurs(id))
        {
            return HResult.NotFound;
        }

        monitor = _monitors.Add(new InstallationMonitor(id ?? PackageId, queue));

        return HResult.Ok;
    }

    /// <summary>XPackageChangeChunkInstallOrder: every chunk is installed, so there is no order to change.</summary>
    public int ChangeChunkInstallOrder(byte* id)
    {
        if (!IsOurs(Utf8Text.Read((nint)id)))
        {
            return HResult.NotFound;
        }

        return HResult.Ok;
    }

    /// <summary>Fully installed and launchable; the three byte counts carry the same value, so their order does not matter.</summary>
    public void GetInstallationProgress(nint monitor, byte* progress)
    {
        if (progress == null || _monitors.Get(monitor) is null)
        {
            return;
        }

        *(ulong*)(progress + PackageLayout.ProgressTotalOffset) = PackageLayout.ReportedPackageBytes;
        *(ulong*)(progress + PackageLayout.ProgressInstalledOffset) = PackageLayout.ReportedPackageBytes;
        *(ulong*)(progress + PackageLayout.ProgressLaunchOffset) = PackageLayout.ReportedPackageBytes;
        progress[PackageLayout.ProgressLaunchableOffset] = GdkModule.True;
        progress[PackageLayout.ProgressCompletedOffset] = GdkModule.True;
    }

    public void CloseMonitor(nint monitor)
    {
        _monitors.Remove(monitor);
    }

    /// <summary>
    /// XPackageRegisterInstallationProgressChanged: the install is already complete, so the callback runs once, on the
    /// completion port of the monitor's queue, where it reads the finished progress. Games that wait for that call
    /// before they read the progress (the callback only marks it dirty) would otherwise wait forever.
    /// </summary>
    public int RegisterProgressChanged(nint monitor, nint context, nint callback, ulong* token)
    {
        if (token == null)
        {
            return HResult.Pointer;
        }

        var watched = _monitors.Get(monitor);

        if (watched is null || callback == 0)
        {
            return HResult.InvalidArgument;
        }

        *token = (ulong)Interlocked.Increment(ref _token);
        var queue = _taskQueues.ResolveQueue(watched.Queue);

        if (queue is null)
        {
            return HResult.Ok;
        }

        return queue.Completion.Submit(new TaskEntry(0, 0, new ProgressChangedStep(callback, context, monitor)), queue, 0);
    }

    /// <summary>XPackageEnumerateFeatures: the game declares no optional features, so the callback is never called.</summary>
    public int EnumerateFeatures(byte* id, nint callback)
    {
        if (callback == 0)
        {
            return HResult.InvalidArgument;
        }

        if (!IsOurs(Utf8Text.Read((nint)id)))
        {
            return HResult.NotFound;
        }

        return HResult.Ok;
    }

    /// <summary>XPackageEstimateDownloadSize: everything is installed, so nothing is left to download.</summary>
    public int EstimateDownloadSize(byte* id, ulong* size, byte* confirm)
    {
        if (size == null)
        {
            return HResult.Pointer;
        }

        if (!IsOurs(Utf8Text.Read((nint)id)))
        {
            return HResult.NotFound;
        }

        *size = 0;

        if (confirm != null)
        {
            *confirm = GdkModule.False;
        }

        return HResult.Ok;
    }

    /// <summary>XPackageGetWriteStats: no install writes, so interval, budget, elapsed and bytes written are all 0.</summary>
    public static int GetWriteStats(byte* stats)
    {
        if (stats == null)
        {
            return HResult.Pointer;
        }

        new Span<byte>(stats, PackageLayout.WriteStatsSize).Clear();

        return HResult.Ok;
    }

    private static void WriteDetails(byte* details, nint[] pointers, Version version)
    {
        new Span<byte>(details, PackageLayout.DetailsSize).Clear();
        *(nint*)(details + PackageLayout.IdentifierOffset) = pointers[(int)PackageText.Identifier];
        *(ulong*)(details + PackageLayout.VersionOffset) = XVersion.Pack(version);
        *(uint*)(details + PackageLayout.KindOffset) = PackageLayout.KindGame;
        *(nint*)(details + PackageLayout.DisplayNameOffset) = pointers[(int)PackageText.DisplayName];

        *(nint*)(details + PackageLayout.DescriptionOffset) = pointers[(int)PackageText.Description];
        *(nint*)(details + PackageLayout.PublisherOffset) = pointers[(int)PackageText.Publisher];
        *(nint*)(details + PackageLayout.StoreIdOffset) = pointers[(int)PackageText.StoreId];
        *(uint*)(details + PackageLayout.CountOffset) = 1;
        *(nint*)(details + PackageLayout.TitleIdOffset) = pointers[(int)PackageText.TitleId];
    }

    private bool IsOurs(string? id)
    {
        return string.IsNullOrEmpty(id) || string.Equals(id, PackageId, StringComparison.OrdinalIgnoreCase);
    }
}
