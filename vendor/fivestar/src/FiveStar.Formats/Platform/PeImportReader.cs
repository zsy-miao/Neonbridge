// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;

namespace FiveStar.Formats.Platform;

/// <summary>
/// Reads the import and delay-import tables of a PE image. Import descriptors are 20 bytes
/// <c>{lookup table RVA, time stamp, forwarder chain, name RVA, address table RVA}</c> ending with an all-zero one;
/// delay descriptors are 32 bytes <c>{attributes, name, module handle, address table, name table, ...}</c>, whose
/// pointers are RVAs when attribute bit 0 is set and virtual addresses otherwise. Thunks are pointer-sized; the top
/// bit marks an import by ordinal, otherwise the thunk is the RVA of a u16 hint followed by the name.
/// </summary>
public static class PeImportReader
{
    private const int DescriptorSize = 20;
    private const int NameField = 12;
    private const int AddressTableField = 16;
    private const int DelayDescriptorSize = 32;
    private const int DelayNameField = 4;
    private const int DelayAddressTableField = 12;
    private const int DelayNameTableField = 16;
    private const uint DelayRvaBased = 1;
    private const int HintSize = 2;
    private const ulong Ordinal64 = 1UL << 63;
    private const uint Ordinal32 = 1U << 31;
    private const ulong OrdinalMask = 0xFFFF;
    private const int MaxDescriptors = 4096;
    private const int MaxThunks = 65536;

    public static IReadOnlyList<PeImport> Imports(PeImage image)
    {
        var imports = new List<PeImport>();
        var (rva, _) = image.Directory(PeDirectory.Import);

        if (rva == 0)
        {
            return imports;
        }

        for (var index = 0; index < MaxDescriptors; index++)
        {
            var at = (uint)(rva + (index * DescriptorSize));
            var lookup = image.U32At(at);
            var name = image.U32At(at + NameField);
            var addresses = image.U32At(at + AddressTableField);

            if (lookup == 0 && name == 0 && addresses == 0)
            {
                break;
            }

            var table = lookup != 0 ? lookup : addresses;
            var dll = image.AsciiAt(name) ?? string.Empty;
            imports.Add(new PeImport(dll, Thunks(image, table, dll), false, addresses));
        }

        return imports;
    }

    public static IReadOnlyList<PeImport> DelayImports(PeImage image)
    {
        var imports = new List<PeImport>();
        var (rva, _) = image.Directory(PeDirectory.DelayImport);

        if (rva == 0)
        {
            return imports;
        }

        for (var index = 0; index < MaxDescriptors; index++)
        {
            var at = (uint)(rva + (index * DelayDescriptorSize));
            var attributes = image.U32At(at);
            var name = image.U32At(at + DelayNameField);
            var addresses = image.U32At(at + DelayAddressTableField);
            var names = image.U32At(at + DelayNameTableField);

            if (name == 0 && addresses == 0 && names == 0)
            {
                break;
            }

            var bias = (attributes & DelayRvaBased) != 0 ? 0 : (uint)image.ImageBase;
            var table = names != 0 ? names : addresses;
            var dll = image.AsciiAt(name - bias) ?? string.Empty;
            imports.Add(new PeImport(dll, Thunks(image, table - bias, dll), true, addresses - bias));
        }

        return imports;
    }

    private static List<string> Thunks(PeImage image, uint table, string dll)
    {
        var functions = new List<string>();
        var size = image.Is64Bit ? sizeof(ulong) : sizeof(uint);

        for (var index = 0; index < MaxThunks; index++)
        {
            var at = (uint)(table + (index * size));
            ulong thunk = image.Is64Bit ? image.U64At(at) : image.U32At(at);

            if (thunk == 0)
            {
                break;
            }

            var ordinalFlag = image.Is64Bit ? Ordinal64 : Ordinal32;

            if ((thunk & ordinalFlag) != 0)
            {
                var ordinal = thunk & OrdinalMask;
                functions.Add(ImportOrdinalNames.Find(dll, ordinal) ?? "#" + ordinal.ToString(CultureInfo.InvariantCulture));

                continue;
            }

            functions.Add(image.AsciiAt((uint)thunk + HintSize) ?? string.Empty);
        }

        return functions;
    }
}
