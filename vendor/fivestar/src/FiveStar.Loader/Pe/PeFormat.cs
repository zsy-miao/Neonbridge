// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Pe;

/// <summary>Header layout of a PE32+ image, from the Microsoft PE format specification (offsets in bytes).</summary>
internal static class PeFormat
{
    // MS-DOS header: "MZ" signature and e_lfanew.
    public const ushort DosSignature = 0x5A4D;
    public const int DosHeaderSize = 0x40;
    public const int NewHeaderPointerOffset = 0x3C;

    // "PE\0\0" signature, then the COFF file header.
    public const uint NtSignature = 0x00004550;
    public const int NtSignatureSize = 4;
    public const int FileHeaderSize = 20;
    public const int MachineOffset = 0;
    public const int NumberOfSectionsOffset = 2;
    public const int SizeOfOptionalHeaderOffset = 16;
    public const int CharacteristicsOffset = 18;

    /// <summary>IMAGE_FILE_MACHINE_AMD64.</summary>
    public const ushort MachineAmd64 = 0x8664;

    // COFF characteristics.
    public const ushort FileRelocsStripped = 0x0001;
    public const ushort FileExecutableImage = 0x0002;
    public const ushort FileLargeAddressAware = 0x0020;
    public const ushort FileDll = 0x2000;

    // Optional header (PE32+).
    public const ushort Pe32PlusMagic = 0x20B;
    public const int MagicOffset = 0;
    public const int MajorLinkerVersionOffset = 2;
    public const int SizeOfCodeOffset = 4;
    public const int SizeOfInitializedDataOffset = 8;
    public const int AddressOfEntryPointOffset = 16;
    public const int BaseOfCodeOffset = 20;
    public const int ImageBaseOffset = 24;
    public const int SectionAlignmentOffset = 32;
    public const int FileAlignmentOffset = 36;
    public const int MajorOperatingSystemVersionOffset = 40;
    public const int MajorSubsystemVersionOffset = 48;
    public const int SizeOfImageOffset = 56;
    public const int SizeOfHeadersOffset = 60;
    public const int SubsystemOffset = 68;
    public const int DllCharacteristicsOffset = 70;
    public const int SizeOfStackReserveOffset = 72;
    public const int SizeOfStackCommitOffset = 80;
    public const int SizeOfHeapReserveOffset = 88;
    public const int SizeOfHeapCommitOffset = 96;
    public const int NumberOfRvaAndSizesOffset = 108;
    public const int DataDirectoriesOffset = 112;
    public const int DataDirectorySize = 8;
    public const int MaxDataDirectories = 16;
    public const int OptionalHeaderSize = DataDirectoriesOffset + (MaxDataDirectories * DataDirectorySize);

    // IMAGE_SUBSYSTEM_* values.
    public const ushort SubsystemWindowsGui = 2;
    public const ushort SubsystemWindowsCui = 3;

    // IMAGE_DLLCHARACTERISTICS_* values.
    public const ushort DllHighEntropyVa = 0x0020;
    public const ushort DllDynamicBase = 0x0040;
    public const ushort DllNxCompat = 0x0100;

    // Section table.
    public const int SectionHeaderSize = 40;
    public const int SectionNameSize = 8;
    public const int SectionVirtualSizeOffset = 8;
    public const int SectionVirtualAddressOffset = 12;
    public const int SectionSizeOfRawDataOffset = 16;
    public const int SectionPointerToRawDataOffset = 20;
    public const int SectionCharacteristicsOffset = 36;
}
