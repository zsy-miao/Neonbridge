// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.Gdk;

/// <summary>
/// x64 layouts of XPackage.h:
/// <c>struct XPackageDetails { const char* packageIdentifier; XVersion version; XPackageKind kind; const char* displayName;
/// const char* description; const char* publisher; const char* storeId; bool installing; uint32_t index; uint32_t count;
/// bool ageRestricted; const char* titleId; }</c> (the members after "installing" are unconfirmed) and
/// <c>struct XPackageInstallationProgress { uint64_t totalBytes; uint64_t installedBytes; uint64_t launchBytes; bool launchable; bool completed; }</c>.
/// </summary>
internal static class PackageLayout
{
    public const int DetailsSize = 80;
    public const int IdentifierOffset = 0;
    public const int VersionOffset = 8;
    public const int KindOffset = 16;
    public const int DisplayNameOffset = 24;
    public const int DescriptionOffset = 32;
    public const int PublisherOffset = 40;
    public const int StoreIdOffset = 48;
    public const int InstallingOffset = 56;
    public const int IndexOffset = 60;
    public const int CountOffset = 64;
    public const int AgeRestrictedOffset = 68;
    public const int TitleIdOffset = 72;

    public const int ProgressTotalOffset = 0;
    public const int ProgressInstalledOffset = 8;
    public const int ProgressLaunchOffset = 16;
    public const int ProgressLaunchableOffset = 24;
    public const int ProgressCompletedOffset = 25;

    // XPackageKind and XPackageChunkAvailability values.
    public const uint KindGame = 0;
    public const uint ChunkReady = 0;

    /// <summary>XPACKAGE_IDENTIFIER_MAX_LENGTH.</summary>
    public const int IdentifierMaxLength = 33;

    /// <summary>struct XPackageWriteStats { uint64_t interval, budget, elapsed, bytesWritten; }.</summary>
    public const int WriteStatsSize = 32;

    /// <summary>The size every installation monitor reports as fully installed: 1 GiB.</summary>
    public const ulong ReportedPackageBytes = 1UL << 30;
}
