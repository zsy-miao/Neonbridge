// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Buffers.Binary;
using System.Text;

namespace FiveStar.Shaders.Containers;

/// <summary>
/// Reads the signature elements of a PSV0 chunk (DxilPipelineStateValidation). After the runtime info (see
/// <see cref="PipelineStateParser"/>): u32 resource count (then u32 bind-info size and the bind infos when non-zero),
/// u32 string-table size and the strings, u32 semantic-index count and the indices, then, when the runtime info counts
/// any elements, u32 element size and the input, output and patch-constant elements.
/// </summary>
public static class PsvSignatureParser
{
    /// <summary>The PSV0 chunk code.</summary>
    public const string ChunkCode = "PSV0";

    private const int DwordSize = 4;

    // PSVSignatureElement0: u32 name offset @0, u32 semantic-index offset @4, u8 rows @8, u8 start row @9,
    // u8 columns:4 start column:2 allocated:1 @10, u8 kind @11, u8 component type @12, u8 interpolation @13.
    private const int IndexOffsetField = 4;
    private const int RowsField = 8;
    private const int StartRowField = 9;
    private const int ColumnsField = 10;
    private const int KindField = 11;
    private const int ComponentTypeField = 12;
    private const int InterpolationField = 13;
    private const int ColumnCountMask = 0xF;
    private const int StartColumnShift = 4;
    private const int StartColumnMask = 0x3;
    private const int AllocatedBit = 6;

    /// <summary>The signatures of a compiled program's PSV0 chunk (a DXBC container as dxc writes it).</summary>
    public static PsvSignatures OfProgram(byte[] program)
    {
        var chunk = DxbcContainer.Parse(program).Find(ChunkCode) ?? throw new InvalidDataException("no PSV0 chunk");

        return Parse(chunk.Data.Span);
    }

    public static PsvSignatures Parse(ReadOnlySpan<byte> chunk)
    {
        var info = PipelineStateParser.Parse(chunk);
        var inputs = info.SignatureInputCount ?? throw new InvalidDataException("PSV0 runtime info has no signature counts");
        var outputs = info.SignatureOutputCount ?? 0;
        var patch = info.SignaturePatchConstantOrPrimitiveCount ?? 0;

        var at = DwordSize + info.InfoSize;
        var resources = ReadInt(chunk, at);
        at += DwordSize;

        if (resources > 0)
        {
            at += DwordSize + (resources * ReadInt(chunk, at));
        }

        var stringsSize = ReadInt(chunk, at);
        var strings = chunk.Slice(at + DwordSize, stringsSize);
        at += DwordSize + stringsSize;

        var indexCount = ReadInt(chunk, at);
        var indices = chunk.Slice(at + DwordSize, indexCount * DwordSize);
        at += DwordSize + (indexCount * DwordSize);
        var elements = new List<PsvElement>();

        if (inputs + outputs + patch > 0)
        {
            var elementSize = ReadInt(chunk, at);
            at += DwordSize;

            for (var index = 0; index < inputs + outputs; index++)
            {
                elements.Add(ReadElement(chunk[(at + (index * elementSize))..], strings, indices));
            }
        }

        return new PsvSignatures(elements.Take(inputs).ToList(), elements.Skip(inputs).ToList());
    }

    private static PsvElement ReadElement(ReadOnlySpan<byte> element, ReadOnlySpan<byte> strings, ReadOnlySpan<byte> indices)
    {
        var nameOffset = ReadInt(element, 0);
        var nameEnd = strings[nameOffset..].IndexOf((byte)0);
        var name = Encoding.Latin1.GetString(strings.Slice(nameOffset, Math.Max(nameEnd, 0)));
        var semanticIndex = ReadInt(indices, ReadInt(element, IndexOffsetField) * DwordSize);
        var columns = element[ColumnsField];

        return new PsvElement(
            name,
            semanticIndex,
            element[RowsField],
            element[StartRowField],
            columns & ColumnCountMask,
            (columns >> StartColumnShift) & StartColumnMask,
            ((columns >> AllocatedBit) & 1) != 0,
            (PsvSemanticKind)element[KindField],
            element[ComponentTypeField],
            element[InterpolationField]);
    }

    private static int ReadInt(ReadOnlySpan<byte> bytes, int at)
    {
        return BinaryPrimitives.ReadInt32LittleEndian(bytes[at..]);
    }
}
