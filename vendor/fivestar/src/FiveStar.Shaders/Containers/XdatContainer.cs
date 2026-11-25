// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Buffers.Binary;
using System.IO.Compression;
using System.Text;

namespace FiveStar.Shaders.Containers;

/// <summary>
/// The Xbox XDAT wrapper that replaces the LLVM bitcode in the DXIL chunk of a game shader container.
/// Section 0x0D holds the zlib-compressed native FSSM program, 0x07 the "NOBC" marker and 0x12 the sizes.
/// </summary>
public sealed class XdatContainer
{
    /// <summary>Wrapper magic.</summary>
    public const string Magic = "XDAT";

    /// <summary>Section id of the zlib-compressed FSSM program.</summary>
    public const int ProgramSection = 0x0D;

    /// <summary>Offset of the XDAT wrapper inside the DXIL chunk (after the 24-byte DXIL program header).</summary>
    public const int OffsetInDxilChunk = 0x18;

    // XDAT header: magic, u32 section count @4, u32 header size @8, then {u32 id, u32 offset, u32 size} from 0x0C.
    private const int CountField = 4;
    private const int HeaderSizeField = 8;
    private const int TableStart = 0x0C;
    private const int EntrySize = 12;
    private const int EntryOffsetField = 4;
    private const int EntrySizeField = 8;
    private const int MagicSize = 4;

    private readonly List<XdatSection> _sections;

    private XdatContainer(int headerSize, List<XdatSection> sections)
    {
        HeaderSize = headerSize;
        _sections = sections;
    }

    public int HeaderSize { get; }

    public IReadOnlyList<XdatSection> Sections => _sections;

    /// <summary>Reads the XDAT wrapper found in the first DXIL chunk of a DXBC container.</summary>
    public static XdatContainer FromDxbc(DxbcContainer container)
    {
        var dxil = container.FindFirst("DXIL") ?? throw new InvalidDataException("no DXIL chunk");

        return Parse(dxil.Data[OffsetInDxilChunk..]);
    }

    /// <summary>Reads an XDAT wrapper.</summary>
    public static XdatContainer Parse(ReadOnlyMemory<byte> bytes)
    {
        var span = bytes.Span;

        if (span.Length < MagicSize || Encoding.Latin1.GetString(span[..MagicSize]) != Magic)
        {
            throw new InvalidDataException("no XDAT");
        }

        var count = BinaryPrimitives.ReadInt32LittleEndian(span[CountField..]);
        var sections = new List<XdatSection>(count);

        for (var index = 0; index < count; index++)
        {
            var entry = TableStart + (index * EntrySize);
            var id = BinaryPrimitives.ReadInt32LittleEndian(span[entry..]);
            var offset = BinaryPrimitives.ReadInt32LittleEndian(span[(entry + EntryOffsetField)..]);
            var size = BinaryPrimitives.ReadInt32LittleEndian(span[(entry + EntrySizeField)..]);
            var length = Math.Clamp(bytes.Length - offset, 0, size);
            sections.Add(new XdatSection(id, bytes.Slice(Math.Min(offset, bytes.Length), length)));
        }

        return new XdatContainer(BinaryPrimitives.ReadInt32LittleEndian(span[HeaderSizeField..]), sections);
    }

    /// <summary>The last section with this id (later entries win), or null.</summary>
    public XdatSection? Find(int id)
    {
        return _sections.LastOrDefault(section => section.Id == id);
    }

    /// <summary>The decompressed native program (section 0x0D).</summary>
    public byte[] ReadProgram()
    {
        var section = Find(ProgramSection) ?? throw new InvalidDataException("no FSSM section");

        return Inflate(section.Data);
    }

    /// <summary>Decompresses a zlib stream.</summary>
    public static byte[] Inflate(ReadOnlyMemory<byte> compressed)
    {
        using var input = new MemoryStream(compressed.ToArray());
        using var zlib = new ZLibStream(input, CompressionMode.Decompress);
        using var output = new MemoryStream();
        zlib.CopyTo(output);

        return output.ToArray();
    }
}
