// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Formats.Common;
using FiveStar.Formats.Rpf;

namespace FiveStar.Formats.Pso;

/// <summary>
/// Reads an L-format PSO file. <c>LSCH</c> holds n struct headers of 0x20 bytes followed by all member records
/// (16 bytes each), <c>LENM</c> n enum headers of 0x18 bytes followed by all values (16 bytes each); n is the count
/// for which the header and record sizes add up to the chunk size. <c>PSIN</c> starts with a block table of
/// 16-byte rows <c>{u32 type, u32 size, u64 0xAAAAAAAAAAAAAAAA}</c>, then the blocks, each 16-byte aligned.
/// <c>LRSC</c> holds a <c>PRD0</c> header whose u32 at +0x10 is the root block id.
/// </summary>
public static class PsoFileReader
{
    private const int ChunkHeader = PsoFraming.ChunkHeaderSize;
    private const int StructHeaderSize = 0x20;
    private const int EnumHeaderSize = 0x18;
    private const int RecordSize = 0x10;
    private const int Alignment = 16;
    private const int BlockRowSize = 16;
    private const int BlockMarkerOffset = 8;
    private const int BlockMarkerSize = 8;
    private const byte RuntimeFill = 0xAA;
    private const int RootIdOffset = 0x10;

    // Struct header: u32 name, u32 signature, u32 flags, u32 0, u64 runtime, u32 size, u16 0, u16 member count.
    private const int SignatureField = 4;
    private const int FlagsField = 8;
    private const int SizeField = 0x18;
    private const int MemberCountField = 0x1E;

    // Enum header: u32 name, u32 signature, u64 runtime, u32 value count, u32 flags.
    private const int EnumCountField = 0x10;

    // Member record: u32 name, u32 offset, u32 type, u32 ref. Enum value: i64 value, u32 name hash, u32 0.
    private const int MemberOffsetField = 4;
    private const int MemberTypeField = 8;
    private const int MemberRefField = 12;
    private const int ValueHashField = 8;

    public static PsoDocument Read(byte[] data)
    {
        var chunks = PsoFraming.Chunks(data);
        var byTag = new Dictionary<string, PsoChunk>(StringComparer.Ordinal);

        foreach (var chunk in chunks)
        {
            byTag[chunk.Tag] = chunk;
        }

        if (!byTag.TryGetValue("PSIN", out var psin) || !byTag.TryGetValue("LSCH", out var schema))
        {
            throw new PsoException("not an L-format PSIN (classic PMAP/PSCH files: see classic_map())", false);
        }

        var structs = ReadStructs(data, schema);
        var enums = new OrderedMap<uint, IReadOnlyList<PsoEnumValue>>();

        if (byTag.TryGetValue("LENM", out var enumChunk))
        {
            enums = ReadEnums(data, enumChunk);
        }

        var (blocks, dataEnd) = ReadBlocks(data, psin);
        long? root = null;

        if (byTag.TryGetValue("LRSC", out var resource))
        {
            root = ReadRoot(data, resource);
        }

        return new PsoDocument(PsoLayout.File, data, chunks, structs, enums, blocks, root, dataEnd, psin.Offset + psin.Size);
    }

    private static OrderedMap<uint, PsoStruct> ReadStructs(byte[] data, PsoChunk chunk)
    {
        var headers = SolveHeaderCount(data, chunk, StructHeaderSize, MemberCountField, sizeof(ushort), "LSCH");
        var structs = new OrderedMap<uint, PsoStruct>();
        long record = chunk.Offset + ChunkHeader + ((long)StructHeaderSize * headers.Count);

        foreach (var header in headers)
        {
            var members = new List<PsoMember>(header.Count);

            for (var index = 0; index < header.Count; index++)
            {
                PsoBytes.Check(data, record, RecordSize);
                members.Add(ReadMember(data, record, index));
                record += RecordSize;
            }

            var name = PsoBytes.U32(data, header.At);
            var signature = PsoBytes.U32(data, header.At + SignatureField);
            var flags = PsoBytes.U32(data, header.At + FlagsField);
            var size = PsoBytes.U32(data, header.At + SizeField);
            structs.Set(name, new PsoStruct(name, signature, flags, size, members));
        }

        return structs;
    }

    /// <summary>Reads one 16-byte member record.</summary>
    internal static PsoMember ReadMember(byte[] data, long at, int index)
    {
        return new PsoMember(
            PsoBytes.U32(data, at),
            PsoBytes.U32(data, at + MemberOffsetField),
            PsoBytes.U32(data, at + MemberTypeField),
            PsoBytes.U32(data, at + MemberRefField),
            index);
    }

    /// <summary>Reads one 16-byte enum value.</summary>
    internal static PsoEnumValue ReadEnumValue(byte[] data, long at)
    {
        PsoBytes.Check(data, at, RecordSize - sizeof(uint));
        var value = (long)PsoBytes.U64(data, at);

        return new PsoEnumValue(PsoBytes.U32(data, at + ValueHashField), value);
    }

    private static OrderedMap<uint, IReadOnlyList<PsoEnumValue>> ReadEnums(byte[] data, PsoChunk chunk)
    {
        var enums = new OrderedMap<uint, IReadOnlyList<PsoEnumValue>>();

        if (chunk.Size == ChunkHeader)
        {
            return enums;
        }

        var headers = SolveHeaderCount(data, chunk, EnumHeaderSize, EnumCountField, sizeof(uint), "LENM");
        long record = chunk.Offset + ChunkHeader + ((long)EnumHeaderSize * headers.Count);

        foreach (var header in headers)
        {
            var values = new List<PsoEnumValue>(header.Count);

            for (var index = 0; index < header.Count; index++)
            {
                values.Add(ReadEnumValue(data, record));
                record += RecordSize;
            }

            enums.Set(PsoBytes.U32(data, header.At), values);
        }

        return enums;
    }

    // The header count n is the first for which 8 + n * header + 16 * (sum of record counts) equals the chunk size.
    private static List<CountedHeader> SolveHeaderCount(byte[] data, PsoChunk chunk, int headerSize, int countField, int countSize, string tag)
    {
        var headers = new List<CountedHeader>();
        long records = 0;

        while (ChunkHeader + ((long)headerSize * (headers.Count + 1)) <= chunk.Size)
        {
            long at = chunk.Offset + ChunkHeader + ((long)headerSize * headers.Count);
            PsoBytes.Check(data, at, headerSize);
            var count = countSize == sizeof(ushort) ? PsoBytes.U16(data, at + countField) : PsoBytes.U32(data, at + countField);
            headers.Add(new CountedHeader(at, (int)count));
            records += count;

            if (ChunkHeader + ((long)headerSize * headers.Count) + (RecordSize * records) == chunk.Size)
            {
                return headers;
            }
        }

        throw new PsoException(tag + " count not solvable", false);
    }

    private static (List<PsoBlock> Blocks, long DataEnd) ReadBlocks(byte[] data, PsoChunk psin)
    {
        var rows = new List<(uint Type, uint Size)>();
        long at = psin.Offset + ChunkHeader;

        while (at + BlockRowSize <= psin.Offset + psin.Size && IsRuntimeMarker(data, at + BlockMarkerOffset))
        {
            rows.Add((PsoBytes.U32(data, at), PsoBytes.U32(data, at + sizeof(uint))));
            at += BlockRowSize;
        }

        var blocks = new List<PsoBlock>(rows.Count);
        var position = Align(at);

        foreach (var (type, size) in rows)
        {
            position = Align(position);
            blocks.Add(new PsoBlock(type, position, size));
            position += size;
        }

        return (blocks, position);
    }

    private static bool IsRuntimeMarker(byte[] data, long at)
    {
        var marker = PsoBytes.Slice(data, at, BlockMarkerSize);

        return marker.Length == BlockMarkerSize && marker.IndexOfAnyExcept(RuntimeFill) < 0;
    }

    private static long? ReadRoot(byte[] data, PsoChunk resource)
    {
        var magic = PsoBytes.Slice(data, resource.Offset + ChunkHeader, sizeof(uint));

        if (!magic.SequenceEqual(BlobClassifier.Prd0Magic))
        {
            return null;
        }

        return PsoBytes.U32(data, resource.Offset + ChunkHeader + RootIdOffset);
    }

    private static long Align(long offset)
    {
        return (offset + Alignment - 1) & ~(long)(Alignment - 1);
    }

    private readonly record struct CountedHeader(long At, int Count);
}
