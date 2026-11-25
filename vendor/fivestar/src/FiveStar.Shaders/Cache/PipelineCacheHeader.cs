// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Buffers.Binary;
using System.Text;

namespace FiveStar.Shaders.Cache;

/// <summary>The 0x80-byte header of the driver's pipeline cache: identity strings, then section counts and sizes.</summary>
/// <param name="CacheId">ASCII text at 0x04, e.g. <c>0x65F41EDD</c>.</param>
/// <param name="DriverVersion">ASCII text at 0x24, e.g. <c>930.048.020</c>.</param>
/// <param name="Stamp">The u64 at 0x48.</param>
/// <param name="GraphicsCount">Graphics pipeline records (0x70 bytes each).</param>
/// <param name="ComputeCount">Compute pipeline records (0x18 bytes each).</param>
/// <param name="MetaCount">Entries of the metadata index.</param>
/// <param name="ShaderCount">Entries of the shader index.</param>
/// <param name="MetaBytes">Size of the metadata area.</param>
/// <param name="CodeBytes">Size of the machine-code area.</param>
public sealed record PipelineCacheHeader(
    string CacheId,
    string DriverVersion,
    ulong Stamp,
    int GraphicsCount,
    int ComputeCount,
    int MetaCount,
    int ShaderCount,
    long MetaBytes,
    long CodeBytes)
{
    /// <summary>Size of the header.</summary>
    public const int Size = 0x80;

    /// <summary>Size of a graphics pipeline record.</summary>
    public const int GraphicsRecordSize = 0x70;

    /// <summary>Size of a compute pipeline record.</summary>
    public const int ComputeRecordSize = 0x18;

    /// <summary>Size of a metadata or shader index entry: u32 offset, u32 size.</summary>
    public const int IndexEntrySize = 8;

    // The two strings fill 0x04..0x24 and 0x24..0x48.
    private const int CacheIdField = 0x04;
    private const int DriverVersionField = 0x24;
    private const int StampField = 0x48;
    private const int GraphicsCountField = 0x50;
    private const int ComputeCountField = 0x54;
    private const int MetaCountField = 0x58;
    private const int ShaderCountField = 0x5C;
    private const int MetaBytesField = 0x60;
    private const int CodeBytesField = 0x64;

    // Magic at 0x00: "PS", 0x00, 0x09.
    private static readonly byte[] MagicBytes = [(byte)'P', (byte)'S', 0x00, 0x09];

    /// <summary>Gets the four magic bytes.</summary>
    public static ReadOnlySpan<byte> Magic => MagicBytes;

    /// <summary>Gets the offset of the first compute record; the graphics records start right after the header.</summary>
    public long ComputeOffset => Size + ((long)GraphicsCount * GraphicsRecordSize);

    /// <summary>Gets the offset of the metadata index.</summary>
    public long MetaIndexOffset => ComputeOffset + ((long)ComputeCount * ComputeRecordSize);

    /// <summary>Gets the offset of the shader index.</summary>
    public long ShaderIndexOffset => MetaIndexOffset + ((long)MetaCount * IndexEntrySize);

    /// <summary>Gets the offset of the metadata area; index offsets count from here.</summary>
    public long MetaOffset => ShaderIndexOffset + ((long)ShaderCount * IndexEntrySize);

    /// <summary>Gets the offset of the machine-code area; shader index offsets count from here.</summary>
    public long CodeOffset => MetaOffset + MetaBytes;

    /// <summary>Gets the size of the whole cache.</summary>
    public long TotalSize => CodeOffset + CodeBytes;

    /// <summary>Whether the bytes start with the cache magic.</summary>
    public static bool HasMagic(ReadOnlySpan<byte> bytes)
    {
        return bytes.StartsWith(MagicBytes);
    }

    /// <summary>Decodes the header at the start of <paramref name="bytes"/>.</summary>
    public static PipelineCacheHeader Parse(ReadOnlySpan<byte> bytes)
    {
        if (bytes.Length < Size || !HasMagic(bytes))
        {
            throw new InvalidDataException("not a pipeline cache (magic PS 00 09)");
        }

        return new PipelineCacheHeader(
            Text(bytes[CacheIdField..DriverVersionField]),
            Text(bytes[DriverVersionField..StampField]),
            BinaryPrimitives.ReadUInt64LittleEndian(bytes[StampField..]),
            Count(bytes, GraphicsCountField),
            Count(bytes, ComputeCountField),
            Count(bytes, MetaCountField),
            Count(bytes, ShaderCountField),
            BinaryPrimitives.ReadUInt32LittleEndian(bytes[MetaBytesField..]),
            BinaryPrimitives.ReadUInt32LittleEndian(bytes[CodeBytesField..]));
    }

    private static int Count(ReadOnlySpan<byte> bytes, int field)
    {
        var value = BinaryPrimitives.ReadInt32LittleEndian(bytes[field..]);

        if (value < 0)
        {
            throw new InvalidDataException($"pipeline cache count at 0x{field:X} is negative");
        }

        return value;
    }

    private static string Text(ReadOnlySpan<byte> field)
    {
        var end = field.IndexOf((byte)0);

        if (end >= 0)
        {
            field = field[..end];
        }

        return Encoding.ASCII.GetString(field);
    }
}
