// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Formats.Common;
using FiveStar.Formats.Rpf;

namespace FiveStar.Formats.Pso;

/// <summary>
/// Reads a PRD0 paged resource: a virtual segment (pointers 0x5xxxxxxx) whose root object is a parser-resource
/// header holding the same struct, enum and block tables as an L-format PSO file. Header layout:
/// +0x10 <c>PRD0</c>, +0x20 u32 root block id, +0x28/+0x30/+0x38/+0x40 virtual pointers to the struct headers,
/// enum headers, block table and string namespaces, +0x48 u16 struct count, enum count, block count.
/// Struct and enum headers carry a virtual pointer to their member or value list in their runtime slot; block rows
/// are <c>{u32 type, u32 size, u64 address}</c>, the address virtual (0x5...) or physical (0x6...).
/// </summary>
public static class PsoResourceReader
{
    private const int MinimumSize = 0x60;
    private const int MagicOffset = 0x10;
    private const int RootOffset = 0x20;
    private const int StructTableField = 0x28;
    private const int EnumTableField = 0x30;
    private const int BlockTableField = 0x38;
    private const int CountsField = 0x48;
    private const int EnumCountOffset = 0x4A;
    private const int BlockCountOffset = 0x4C;
    private const int StructHeaderSize = 0x20;
    private const int EnumHeaderSize = 0x18;
    private const int BlockRowSize = 0x10;
    private const int RecordSize = 0x10;
    private const int TablePointersSize = 0x20;
    private const int CountsSize = 8;

    // Struct header fields (as in LSCH, with the member-list pointer in the runtime slot at +0x10).
    private const int SignatureField = 4;
    private const int FlagsField = 8;
    private const int ListPointerField = 0x10;
    private const int SizeField = 0x18;
    private const int MemberCountField = 0x1E;

    // Enum header: u32 name, u32 signature, u64 value-list pointer, u32 count, u32 flags.
    private const int EnumListField = 8;
    private const int EnumCountField = 0x10;

    // Pointers: top nibble of the 32-bit address names the segment.
    private const int SegmentShift = 28;
    private const ulong VirtualSegment = 5;
    private const ulong PhysicalSegment = 6;
    private const ulong AddressMask = (1UL << SegmentShift) - 1;

    public static PsoDocument Read(byte[] data)
    {
        if (data.Length < MinimumSize || !BlobClassifier.HasPrd0Magic(data))
        {
            throw new PsoException("no PRD0 header at +0x10", false);
        }

        var root = PsoBytes.U32(data, RootOffset);
        PsoBytes.Check(data, StructTableField, TablePointersSize);
        PsoBytes.Check(data, CountsField, CountsSize);
        var structCount = PsoBytes.U16(data, CountsField);
        var enumCount = PsoBytes.U16(data, EnumCountOffset);
        var blockCount = PsoBytes.U16(data, BlockCountOffset);

        var structs = ReadStructs(data, PsoBytes.U64(data, StructTableField), structCount);
        var enums = ReadEnums(data, PsoBytes.U64(data, EnumTableField), enumCount);
        var blocks = ReadBlocks(data, PsoBytes.U64(data, BlockTableField), blockCount);

        return new PsoDocument(PsoLayout.Resource, data, [], structs, enums, blocks, root, 0, 0);
    }

    /// <summary>The buffer offset of a virtual pointer; throws for anything else.</summary>
    public static long VirtualOffset(ulong address, int length)
    {
        if (address >> SegmentShift != VirtualSegment || (long)(address & AddressMask) > length)
        {
            throw new PsoException("bad virtual pointer " + DecimalText.Hex((long)address), false);
        }

        return (long)(address & AddressMask);
    }

    private static OrderedMap<uint, PsoStruct> ReadStructs(byte[] data, ulong table, int count)
    {
        var structs = new OrderedMap<uint, PsoStruct>();

        for (var index = 0; index < count; index++)
        {
            var at = VirtualOffset(table, data.Length) + ((long)StructHeaderSize * index);
            PsoBytes.Check(data, at, StructHeaderSize);
            var name = PsoBytes.U32(data, at);
            int memberCount = PsoBytes.U16(data, at + MemberCountField);
            long list = 0;

            if (memberCount != 0)
            {
                list = VirtualOffset(PsoBytes.U64(data, at + ListPointerField), data.Length);
            }

            var members = new List<PsoMember>(memberCount);

            for (var member = 0; member < memberCount; member++)
            {
                PsoBytes.Check(data, list + (RecordSize * member), RecordSize);
                members.Add(PsoFileReader.ReadMember(data, list + (RecordSize * member), member));
            }

            var schema = new PsoStruct(
                name,
                PsoBytes.U32(data, at + SignatureField),
                PsoBytes.U32(data, at + FlagsField),
                PsoBytes.U32(data, at + SizeField),
                members);
            structs.Set(name, schema);
        }

        return structs;
    }

    private static OrderedMap<uint, IReadOnlyList<PsoEnumValue>> ReadEnums(byte[] data, ulong table, int count)
    {
        var enums = new OrderedMap<uint, IReadOnlyList<PsoEnumValue>>();

        for (var index = 0; index < count; index++)
        {
            var at = VirtualOffset(table, data.Length) + ((long)EnumHeaderSize * index);
            PsoBytes.Check(data, at, EnumHeaderSize);
            var valueCount = (int)PsoBytes.U32(data, at + EnumCountField);
            long list = 0;

            if (valueCount != 0)
            {
                list = VirtualOffset(PsoBytes.U64(data, at + EnumListField), data.Length);
            }

            var values = new List<PsoEnumValue>(valueCount);

            for (var value = 0; value < valueCount; value++)
            {
                values.Add(PsoFileReader.ReadEnumValue(data, list + (RecordSize * value)));
            }

            enums.Set(PsoBytes.U32(data, at), values);
        }

        return enums;
    }

    private static List<PsoBlock> ReadBlocks(byte[] data, ulong table, int count)
    {
        var blocks = new List<PsoBlock>(count);

        for (var index = 0; index < count; index++)
        {
            var at = VirtualOffset(table, data.Length) + ((long)BlockRowSize * index);
            PsoBytes.Check(data, at, BlockRowSize);
            var type = PsoBytes.U32(data, at);
            var size = PsoBytes.U32(data, at + sizeof(uint));
            var address = PsoBytes.U64(data, at + sizeof(ulong));

            if (address >> SegmentShift == PhysicalSegment)
            {
                blocks.Add(new PsoBlock(type, null, size));

                continue;
            }

            blocks.Add(new PsoBlock(type, VirtualOffset(address, data.Length), size));
        }

        return blocks;
    }
}
