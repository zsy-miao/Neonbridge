// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Buffers.Binary;
using System.Text;

namespace FiveStar.Shaders.Dxil.Container;

/// <summary>
/// ISG1, OSG1 and PSG1 as dxc writes them: count, offset 8, one 32-byte row per element sorted by register (no
/// register last, then by name), then each distinct name once in element order.
/// </summary>
public static class SignatureWriter
{
    private const int HeaderBytes = 8;
    private const int ElementBytes = 32;
    private const int FullMask = 0xF;

    // DxilProgramSignatureElement: stream @0, name offset @4, semantic index @8, system value @12, component type @16,
    // register @20, mask @24, read/write mask @25 (inputs: read; outputs: never written), min precision @28.
    private const int NameField = 4;
    private const int IndexField = 8;
    private const int SystemValueField = 12;
    private const int ComponentTypeField = 16;
    private const int RegisterField = 20;
    private const int MaskField = 24;
    private const int ReadWriteField = 25;

    public static byte[] Write(IReadOnlyList<DxilSignatureElement> elements, bool input)
    {
        var listed = elements.Where(element => element.InSignature).ToList();
        var nameStart = HeaderBytes + (ElementBytes * listed.Count);
        var (names, text) = Names(listed, nameStart);

        var bytes = new byte[nameStart + text.Count];
        BinaryPrimitives.WriteUInt32LittleEndian(bytes, (uint)listed.Count);
        BinaryPrimitives.WriteUInt32LittleEndian(bytes.AsSpan(sizeof(uint)), HeaderBytes);
        var ordered = listed.OrderBy(element => (uint)element.StartRow).ThenBy(element => names[element.Name]).ToList();

        for (var index = 0; index < ordered.Count; index++)
        {
            var element = ordered[index];
            var row = bytes.AsSpan(HeaderBytes + (ElementBytes * index), ElementBytes);
            var used = element.Usage << Math.Max(element.StartColumn, 0);
            var readWrite = used;

            if (!input)
            {
                readWrite = ~used & FullMask;
            }

            BinaryPrimitives.WriteUInt32LittleEndian(row[NameField..], (uint)names[element.Name]);
            BinaryPrimitives.WriteUInt32LittleEndian(row[IndexField..], (uint)element.Index);
            BinaryPrimitives.WriteUInt32LittleEndian(row[SystemValueField..], (uint)element.SystemValue);

            BinaryPrimitives.WriteUInt32LittleEndian(row[ComponentTypeField..], element.RegisterComponentType);
            BinaryPrimitives.WriteInt32LittleEndian(row[RegisterField..], element.StartRow);
            row[MaskField] = element.Mask;
            row[ReadWriteField] = (byte)readWrite;
        }

        text.CopyTo(bytes, nameStart);

        return bytes;
    }

    /// <summary>Each distinct name once, in element order, padded to whole dwords; and where each starts in the part.</summary>
    private static (Dictionary<string, int> Offsets, List<byte> Text) Names(List<DxilSignatureElement> elements, int start)
    {
        var offsets = new Dictionary<string, int>(StringComparer.Ordinal);
        var text = new List<byte>();

        foreach (var element in elements)
        {
            if (!offsets.ContainsKey(element.Name))
            {
                offsets[element.Name] = start + text.Count;
                text.AddRange(Encoding.ASCII.GetBytes(element.Name));
                text.Add(0);
            }
        }

        while (text.Count % sizeof(uint) != 0)
        {
            text.Add(0);
        }

        return (offsets, text);
    }
}
