// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Pe;

/// <summary>IMAGE_TLS_DIRECTORY64 field offsets (PE format specification, "The TLS Section").</summary>
internal static class TlsFormat
{
    public const int DirectorySize = 40;
    public const int StartAddressOfRawDataOffset = 0;
    public const int EndAddressOfRawDataOffset = 8;
    public const int AddressOfIndexOffset = 16;
    public const int AddressOfCallBacksOffset = 24;
    public const int SizeOfZeroFillOffset = 32;
    public const int CharacteristicsOffset = 36;
}
