// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Formats.Platform;

/// <summary>Names of XVD content types and volume flags (from the public xvdtool definitions).</summary>
public static class XvdNames
{
    /// <summary>Volume flag bits and their names, lowest bit first.</summary>
    public static readonly IReadOnlyList<(uint Bit, string Name)> Flags =
    [
        (0x1, "ReadOnly"),
        (0x2, "EncryptionDisabled"),
        (0x4, "DataIntegrityDisabled"),
        (0x8, "LegacySectorSize"),
        (0x10, "ResiliencyEnabled"),
        (0x20, "SraReadOnly"),
        (0x40, "RegionIdInXts"),
        (0x80, "TitleSpecific"),
        (0x100, "PlaceholderXvd"),
        (0x200, "TrimSupported"),
        (0x400, "DiffDisk"),
    ];

    private static readonly string?[] ContentTypes =
    [
        "Data", "Title", "SystemOS", "EraOS", "Scratch", "ResetData", "Application", "HostOS", "X360STFS", "X360FATX",
        "X360GDFX", "Updater", "OfflineUpdater", "Template", "MteHost", "MteApp", "MteTitle", "MteEraOS", "EraTools",
        "SystemTools", "SystemAux", "AcousticModel", "SystemCodecsVolume", "QasltPackage", "AppDlc", "TitleDlc",
        "UniversalDlc", "SystemDataVolume", "TestVolume", "HardwareTestVolume", "KioskContent", null, "HostProfiler",
        "Uwa", "Unknown34", "Unknown35", "Unknown36", "ServerAgent",
    ];

    /// <summary>The name of a content type, or null when it has none.</summary>
    public static string? ContentTypeName(uint contentType)
    {
        if (contentType >= ContentTypes.Length)
        {
            return null;
        }

        return ContentTypes[contentType];
    }
}
