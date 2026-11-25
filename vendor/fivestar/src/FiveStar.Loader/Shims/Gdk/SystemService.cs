// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Security.Cryptography;
using System.Text;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.Gdk;

/// <summary>XSystem.h, XThread.h, XGame.h, XGameEvent.h and XPersistentLocalStorage.h with offline values.</summary>
internal sealed unsafe class SystemService : IDisposable
{
    /// <summary>XSystemDeviceType::XboxScarlettAnaconda (Series X), the device the shipped build targets.</summary>
    public const uint XboxScarlettAnaconda = 7;

    public const string PromptForSpaceApi = "XPersistentLocalStoragePromptUserForSpaceAsync";

    private const string ConsoleId = "XLOADER.0000.0000.0000.0000.0000.0001";
    private const string SandboxId = "RETAIL";
    private const string DeviceFamily = "Windows.Xbox";
    private const string DeviceForm = "Xbox Series X";
    private const string LocalStorageFolder = "localstorage";

    /// <summary>The first events written are logged; the rest are dropped silently.</summary>
    private const int LoggedEvents = 5;

    /// <summary>Local storage size when MicrosoftGame.config declares none: 2048 MB.</summary>
    private const ulong DefaultLocalStorageBytes = 2048UL << 20;

    // XSystemAnalyticsInfo { XVersion osVersion; XVersion hostingOsVersion; char family[64]; char form[64]; }; array sizes unconfirmed.
    private const int AnalyticsInfoSize = 144;
    private const int AnalyticsHostingVersionOffset = 8;
    private const int AnalyticsFamilyOffset = 16;
    private const int AnalyticsFormOffset = 80;
    private const int AnalyticsTextCapacity = 63;

    // XPersistentLocalStorageSpaceInfo { availableFreeBytes; totalFreeBytes; usedBytes; totalBytes; }; member order unconfirmed.
    private const int SpaceAvailableSlot = 0;
    private const int SpaceFreeSlot = 1;
    private const int SpaceUsedSlot = 2;
    private const int SpaceTotalSlot = 3;

    private readonly ThreadLocal<bool> _timeSensitive = new();
    private readonly GdkConfig _config;
    private readonly NativeBoundary _boundary;
    private int _events;

    public SystemService(GdkConfig config, NativeBoundary boundary)
    {
        _config = config;
        _boundary = boundary;
    }

    public uint DeviceType => _config.Options.DeviceType;

    /// <summary>XSystemAnalyticsInfo XSystemGetAnalyticsInfo(): returned by value through the hidden result pointer.</summary>
    public byte* GetAnalyticsInfo(byte* result)
    {
        var os = _config.Options.OsVersion;
        ulong version = XVersion.Pack(new Version(os.Major, os.Minor, os.Build));
        new Span<byte>(result, AnalyticsInfoSize).Clear();
        *(ulong*)result = version;
        *(ulong*)(result + AnalyticsHostingVersionOffset) = version;
        Encoding.ASCII.GetBytes(DeviceFamily).CopyTo(new Span<byte>(result + AnalyticsFamilyOffset, AnalyticsTextCapacity));

        Encoding.ASCII.GetBytes(DeviceForm).CopyTo(new Span<byte>(result + AnalyticsFormOffset, AnalyticsTextCapacity));

        return result;
    }

    public static int GetConsoleId(nuint size, byte* id, nuint* used)
    {
        return Utf8Text.Write(ConsoleId, size, id, used);
    }

    public static int GetSandboxId(nuint size, byte* id, nuint* used)
    {
        return Utf8Text.Write(SandboxId, size, id, used);
    }

    /// <summary>
    /// XSystemGetAppSpecificDeviceId: Base64 of SHA-256 over the title id and the PC name, 44 characters
    /// (XSystemAppSpecificDeviceIdBytes is 45 with the NUL); stable for one title on one PC, different across titles.
    /// </summary>
    public int GetAppSpecificDeviceId(nuint size, byte* id, nuint* used)
    {
        byte[] hash = SHA256.HashData(Encoding.UTF8.GetBytes($"{_config.TitleIdText}|{Environment.MachineName}"));

        return Utf8Text.Write(Convert.ToBase64String(hash), size, id, used);
    }

    public void SetTimeSensitive(bool value)
    {
        _timeSensitive.Value = value;
    }

    public bool IsTimeSensitive()
    {
        return _timeSensitive.Value;
    }

    public void AssertNotTimeSensitive()
    {
        if (_timeSensitive.Value)
        {
            _boundary.Warn("XThreadAssertNotTimeSensitive on a time-sensitive thread");
        }
    }

    public int GetTitleId(uint* titleId)
    {
        if (titleId == null)
        {
            return HResult.Pointer;
        }

        *titleId = _config.Identity.TitleId;

        return HResult.Ok;
    }

    /// <summary>Events are dropped (offline); the first few are logged.</summary>
    public int WriteEvent(byte* name)
    {
        if (Interlocked.Increment(ref _events) <= LoggedEvents)
        {
            _boundary.Info($"XGameEventWrite({Utf8Text.Read((nint)name)}) dropped (offline)");
        }

        return HResult.Ok;
    }

    public int GetLocalStoragePathSize(nuint* size)
    {
        if (size == null)
        {
            return HResult.Pointer;
        }

        *size = Utf8Text.SizeWithTerminator(LocalStorage());

        return HResult.Ok;
    }

    public int GetLocalStoragePath(nuint size, byte* path, nuint* used)
    {
        return Utf8Text.Write(LocalStorage(), size, path, used);
    }

    public int GetLocalStorageSpaceInfo(ulong* info)
    {
        if (info == null)
        {
            return HResult.Pointer;
        }

        ulong total = LocalStorageTotalBytes();
        ulong used = LocalStorageUsedBytes();
        ulong free = total > used ? total - used : 0;
        info[SpaceAvailableSlot] = free;
        info[SpaceFreeSlot] = free;

        info[SpaceUsedSlot] = used;
        info[SpaceTotalSlot] = total;

        return HResult.Ok;
    }

    /// <summary>Whether <paramref name="requested"/> more bytes fit in the local storage the title declared.</summary>
    public bool LocalStorageFits(ulong requested)
    {
        ulong total = LocalStorageTotalBytes();
        ulong used = LocalStorageUsedBytes();

        return used <= total && requested <= total - used;
    }

    public void Dispose()
    {
        _timeSensitive.Dispose();
    }

    private ulong LocalStorageTotalBytes()
    {
        ulong declared = _config.Identity.PersistentLocalStorageBytes;
        ulong total = declared != 0 ? declared : DefaultLocalStorageBytes;

        return total;
    }

    private ulong LocalStorageUsedBytes()
    {
        return (ulong)new DirectoryInfo(LocalStorage()).EnumerateFiles("*", SearchOption.AllDirectories).Sum(f => f.Length);
    }

    private string LocalStorage()
    {
        _config.Ensure();
        string path = Path.Combine(_config.SaveRoot, LocalStorageFolder);
        Directory.CreateDirectory(path);

        return path;
    }
}
