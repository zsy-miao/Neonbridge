// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Pe;

/// <summary>Index of an optional-header data directory (PE format specification, "Optional Header Data Directories").</summary>
internal enum DataDirectoryIndex
{
    Export = 0,
    Import = 1,
    Resource = 2,
    Exception = 3,
    BaseRelocation = 5,
    Tls = 9,
    LoadConfig = 10,
    ImportAddressTable = 12,
    DelayImport = 13,
}
