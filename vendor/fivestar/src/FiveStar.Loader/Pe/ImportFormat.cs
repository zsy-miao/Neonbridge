// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Pe;

/// <summary>Import and delay-import table layout (PE format specification, ".idata Section" and "Delay-Load Import Tables").</summary>
internal static class ImportFormat
{
    // IMAGE_IMPORT_DESCRIPTOR.
    public const int DescriptorSize = 20;
    public const int OriginalFirstThunkOffset = 0;
    public const int NameOffset = 12;
    public const int FirstThunkOffset = 16;

    /// <summary>A 64-bit thunk; IMAGE_ORDINAL_FLAG64 marks an import by ordinal.</summary>
    public const int ThunkSize = 8;
    public const ulong OrdinalFlag = 0x8000000000000000UL;
    public const ulong OrdinalMask = 0xFFFF;

    /// <summary>IMAGE_IMPORT_BY_NAME.Hint precedes the name.</summary>
    public const int HintSize = 2;

    // IMAGE_DELAYLOAD_DESCRIPTOR: eight 32-bit fields.
    public const int DelayDescriptorFields = 8;
    public const int DelayAttributesField = 0;
    public const int DelayDllNameField = 1;
    public const int DelayModuleHandleField = 2;
    public const int DelayIatField = 3;
    public const int DelayNameTableField = 4;

    /// <summary>dlattrRva: the descriptor's fields are RVAs, not VAs.</summary>
    public const uint DelayRvaBased = 1;
}
