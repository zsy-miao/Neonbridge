// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Buffers.Binary;
using System.Text;

namespace FiveStar.Shaders.Containers;

/// <summary>
/// Rewrites a game container whose DXIL chunk carries LLVM bitcode inside an XDAT wrapper into a standard DXBC
/// container that DirectX tools can read. Containers whose XDAT holds no bitcode (a "NOBC" marker) are refused.
/// </summary>
public static class XdatUnwrapper
{
    // DXIL program header: u32 program version @0, u32 size in dwords @4, "DXIL" @8, u32 DXIL version @12,
    // u32 bitcode offset (from the "DXIL" magic) @16, u32 bitcode size @20.
    private const int DxilVersionField = 12;
    private const int BitcodeOffsetFromMagic = 0x10;
    private const int ProgramHeaderPrefix = 8;

    // DXIL header written in front of the bitcode: "DXIL", u32 version @4, u32 bitcode offset @8, u32 bitcode size @12.
    private const int BodyVersionField = 4;
    private const int BodyOffsetField = 8;
    private const int BodySizeField = 12;

    // Container header fields after the hash (relative to 0x14): u16 major, u16 minor, u32 total size @4, u32 count @8.
    private const int TotalSizeField = 4;
    private const int ChunkCountField = 8;
    private const int ZlibHeaderSize = 2;

    // DXBC header: magic, 16-byte hash, u16 major, u16 minor, u32 total size, u32 chunk count, u32 offsets.
    private const int ContainerHeaderSize = 0x20;
    private const ushort MajorVersion = 1;
    private const int ChunkHeaderSize = 8;
    private const int DwordSize = 4;

    private static readonly byte[] BitcodeMagic = [(byte)'B', (byte)'C', 0xC0, 0xDE];

    /// <summary>The standard container: bitcode unwrapped from XDAT, PRIV chunks dropped, hash zeroed.</summary>
    public static byte[] Unwrap(ReadOnlyMemory<byte> container)
    {
        var parsed = DxbcContainer.Parse(container);
        var chunks = new List<(string FourCc, byte[] Data)>();

        foreach (var chunk in parsed.Chunks)
        {
            if (chunk.FourCc == "PRIV")
            {
                continue;
            }

            var data = chunk.FourCc == "DXIL" ? RebuildProgram(chunk.Data) : chunk.Data.ToArray();
            chunks.Add((chunk.FourCc, data));
        }

        return Assemble(chunks);
    }

    private static byte[] RebuildProgram(ReadOnlyMemory<byte> dxil)
    {
        var span = dxil.Span;
        var programVersion = BinaryPrimitives.ReadUInt32LittleEndian(span);
        var dxilVersion = BinaryPrimitives.ReadUInt32LittleEndian(span[DxilVersionField..]);
        var bitcode = FindBitcode(XdatContainer.Parse(dxil[XdatContainer.OffsetInDxilChunk..]));
        var padded = new byte[(bitcode.Length + DwordSize - 1) / DwordSize * DwordSize];
        bitcode.CopyTo(padded, 0);

        // Program header, then the DXIL header pointing at the bitcode right after it.
        var body = new byte[BitcodeOffsetFromMagic + padded.Length];
        Encoding.Latin1.GetBytes("DXIL").CopyTo(body, 0);
        BinaryPrimitives.WriteUInt32LittleEndian(body.AsSpan(BodyVersionField), dxilVersion);
        BinaryPrimitives.WriteInt32LittleEndian(body.AsSpan(BodyOffsetField), BitcodeOffsetFromMagic);
        BinaryPrimitives.WriteInt32LittleEndian(body.AsSpan(BodySizeField), padded.Length);
        padded.CopyTo(body, BitcodeOffsetFromMagic);

        var program = new byte[ProgramHeaderPrefix + body.Length];
        BinaryPrimitives.WriteUInt32LittleEndian(program, programVersion);
        BinaryPrimitives.WriteInt32LittleEndian(program.AsSpan(DwordSize), program.Length / DwordSize);
        body.CopyTo(program, ProgramHeaderPrefix);

        return program;
    }

    private static byte[] FindBitcode(XdatContainer xdat)
    {
        byte[]? bitcode = null;

        foreach (var section in xdat.Sections)
        {
            if (!IsZlib(section.Data.Span))
            {
                continue;
            }

            var raw = XdatContainer.Inflate(section.Data);

            if (raw.AsSpan().StartsWith(BitcodeMagic))
            {
                bitcode = raw;
            }
        }

        return bitcode ?? throw new InvalidDataException("no bitcode section");
    }

    private static bool IsZlib(ReadOnlySpan<byte> data)
    {
        // RFC 1950 header: CMF 0x78 (deflate, 32 KB window), FLG 0x01 / 0x9C / 0xDA (compression levels).
        const byte DeflateMethod = 0x78;
        const byte FastestLevel = 0x01;
        const byte DefaultLevel = 0x9C;
        const byte BestLevel = 0xDA;

        if (data.Length < ZlibHeaderSize || data[0] != DeflateMethod)
        {
            return false;
        }

        return data[1] is FastestLevel or DefaultLevel or BestLevel;
    }

    private static byte[] Assemble(List<(string FourCc, byte[] Data)> chunks)
    {
        var head = ContainerHeaderSize + (DwordSize * chunks.Count);
        var total = head + chunks.Sum(chunk => ChunkHeaderSize + chunk.Data.Length);
        var output = new byte[total];
        Encoding.Latin1.GetBytes(DxbcContainer.Magic).CopyTo(output, 0);

        // Header after the zero hash: version 1.0, total size, chunk count, chunk offsets.
        var header = output.AsSpan(DxbcContainer.HashOffset + DxbcContainer.HashSize);
        BinaryPrimitives.WriteUInt16LittleEndian(header, MajorVersion);
        BinaryPrimitives.WriteInt32LittleEndian(header[TotalSizeField..], total);
        BinaryPrimitives.WriteInt32LittleEndian(header[ChunkCountField..], chunks.Count);

        var cursor = head;

        for (var index = 0; index < chunks.Count; index++)
        {
            var (fourCc, data) = chunks[index];
            BinaryPrimitives.WriteInt32LittleEndian(output.AsSpan(ContainerHeaderSize + (DwordSize * index)), cursor);
            Encoding.Latin1.GetBytes(fourCc).CopyTo(output, cursor);
            BinaryPrimitives.WriteInt32LittleEndian(output.AsSpan(cursor + DwordSize), data.Length);
            data.CopyTo(output, cursor + ChunkHeaderSize);
            cursor += ChunkHeaderSize + data.Length;
        }

        return output;
    }
}
