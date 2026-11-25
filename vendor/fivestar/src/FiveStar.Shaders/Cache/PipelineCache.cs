// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Buffers.Binary;

namespace FiveStar.Shaders.Cache;

/// <summary>
/// The console driver's pipeline cache (magic <c>PS 00 09</c>): a header, graphics and compute pipeline records, a
/// metadata index and a shader index (u32 offset, u32 size each), the metadata area and the machine-code area.
/// </summary>
public sealed class PipelineCache
{
    // Graphics record: u64 hash @0, u32 state meta @8, u32 input layout meta @C, 16 + 16 + 8 bytes @10/@20/@30,
    // seven slots of {i32 shader, i32 meta} @38 (-1 when empty).
    private const int StateMetaField = 0x08;
    private const int InputLayoutField = 0x0C;
    private const int KeyField = 0x10;
    private const int GroupAField = 0x20;
    private const int GroupBField = 0x30;
    private const int SlotsField = 0x38;
    private const int SlotCount = 7;
    private const int SlotSize = 8;
    private const int WideKeySize = 16;
    private const int NarrowKeySize = 8;

    // Compute record: u64 hash @0, u32 state meta @8, u32 extra meta @C, u32 shader @10, u32 shader meta @14.
    private const int ComputeShaderField = 0x10;
    private const int ComputeShaderMetaField = 0x14;

    private readonly ReadOnlyMemory<byte> _blob;

    private PipelineCache(ReadOnlyMemory<byte> blob, PipelineCacheHeader header)
    {
        _blob = blob;
        Header = header;
        Graphics = Enumerable.Range(0, header.GraphicsCount).Select(ReadGraphics).ToList();
        Compute = Enumerable.Range(0, header.ComputeCount).Select(ReadCompute).ToList();
    }

    public PipelineCacheHeader Header { get; }

    public IReadOnlyList<GraphicsPipelineRecord> Graphics { get; }

    public IReadOnlyList<ComputePipelineRecord> Compute { get; }

    /// <summary>Gets the length of the blob, which is shorter than the header's total when the cache was cut off.</summary>
    public long Length => _blob.Length;

    /// <summary>Reads a cache blob; it may end inside the machine-code area.</summary>
    public static PipelineCache Read(ReadOnlyMemory<byte> blob)
    {
        var header = PipelineCacheHeader.Parse(blob.Span);

        if (header.MetaOffset + header.MetaBytes > blob.Length)
        {
            throw new InvalidDataException("pipeline cache ends before its metadata area does");
        }

        return new PipelineCache(blob, header);
    }

    /// <summary>The bytes of a shader index entry, or null when they are not (all) in the blob.</summary>
    public ReadOnlyMemory<byte>? Shader(int index)
    {
        var (offset, size) = IndexEntry(Header.ShaderIndexOffset, Header.ShaderCount, index);
        var start = Header.CodeOffset + offset;

        if (start + size > _blob.Length)
        {
            return null;
        }

        return _blob.Slice((int)start, (int)size);
    }

    /// <summary>The bytes of a metadata index entry.</summary>
    public ReadOnlyMemory<byte> Meta(int index)
    {
        var (offset, size) = IndexEntry(Header.MetaIndexOffset, Header.MetaCount, index);

        if (offset + size > Header.MetaBytes)
        {
            throw new InvalidDataException($"metadata entry {index} runs past the metadata area");
        }

        return _blob.Slice((int)(Header.MetaOffset + offset), (int)size);
    }

    /// <summary>The fixed-function state registers of a graphics pipeline, or null when its entry has an unknown layout.</summary>
    public GraphicsStateRegisters? State(GraphicsPipelineRecord pipeline)
    {
        return GraphicsStateParser.Parse(Meta(pipeline.StateMeta).Span);
    }

    private (long Offset, long Size) IndexEntry(long table, int count, int index)
    {
        ArgumentOutOfRangeException.ThrowIfNegative(index);
        ArgumentOutOfRangeException.ThrowIfGreaterThanOrEqual(index, count);

        var entry = _blob.Span[(int)(table + ((long)index * PipelineCacheHeader.IndexEntrySize))..];

        return (BinaryPrimitives.ReadUInt32LittleEndian(entry), BinaryPrimitives.ReadUInt32LittleEndian(entry[sizeof(uint)..]));
    }

    private GraphicsPipelineRecord ReadGraphics(int index)
    {
        var record = _blob.Span.Slice((int)(PipelineCacheHeader.Size +((long)index * PipelineCacheHeader.GraphicsRecordSize)), PipelineCacheHeader.GraphicsRecordSize);
        var stages = new List<PipelineStageLink>();

        for (var slot = 0; slot < SlotCount; slot++)
        {
            var shader = BinaryPrimitives.ReadInt32LittleEndian(record[(SlotsField + (slot * SlotSize))..]);
            var meta = BinaryPrimitives.ReadInt32LittleEndian(record[(SlotsField + (slot * SlotSize) + sizeof(int))..]);

            if (shader >= 0)
            {
                stages.Add(new PipelineStageLink((PipelineSlot)slot, shader, meta));
            }
        }

        return new GraphicsPipelineRecord(
            index,
            BinaryPrimitives.ReadUInt64LittleEndian(record),
            BinaryPrimitives.ReadInt32LittleEndian(record[StateMetaField..]),
            BinaryPrimitives.ReadInt32LittleEndian(record[InputLayoutField..]),
            Convert.ToHexStringLower(record.Slice(KeyField, WideKeySize)),
            Convert.ToHexStringLower(record.Slice(GroupAField, WideKeySize)),
            Convert.ToHexStringLower(record.Slice(GroupBField, NarrowKeySize)),
            stages);
    }

    private ComputePipelineRecord ReadCompute(int index)
    {
        var record = _blob.Span.Slice((int)(Header.ComputeOffset + ((long)index * PipelineCacheHeader.ComputeRecordSize)), PipelineCacheHeader.ComputeRecordSize);

        return new ComputePipelineRecord(
            index,
            BinaryPrimitives.ReadUInt64LittleEndian(record),
            BinaryPrimitives.ReadInt32LittleEndian(record[StateMetaField..]),
            BinaryPrimitives.ReadInt32LittleEndian(record[InputLayoutField..]),
            BinaryPrimitives.ReadInt32LittleEndian(record[ComputeShaderField..]),
            BinaryPrimitives.ReadInt32LittleEndian(record[ComputeShaderMetaField..]));
    }
}
