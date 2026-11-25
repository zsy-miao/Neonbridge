// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Pe;

/// <summary>IMAGE_EXPORT_DIRECTORY field offsets (PE format specification, "Export Directory Table").</summary>
internal static class ExportFormat
{
    public const int DirectorySize = 40;
    public const int NameOffset = 12;
    public const int OrdinalBaseOffset = 16;
    public const int NumberOfFunctionsOffset = 20;
    public const int NumberOfNamesOffset = 24;
    public const int AddressOfFunctionsOffset = 28;
    public const int AddressOfNamesOffset = 32;
    public const int AddressOfNameOrdinalsOffset = 36;
}
