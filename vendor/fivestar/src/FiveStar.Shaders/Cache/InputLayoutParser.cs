// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Buffers.Binary;
using System.Text;

namespace FiveStar.Shaders.Cache;

/// <summary>
/// Reads the input-layout metadata of a graphics pipeline: a 0x50-byte header ending in the element count, then packed
/// elements <c>u32 name length (with NUL), name, u32 semantic index, format, input slot, offset, classification, step rate</c>.
/// </summary>
public static class InputLayoutParser
{
    private const int CountField = 0x4C;
    private const int FirstElement = 0x50;
    private const int FieldCount = 6;
    private const int FieldSize = 4;

    // Five of 0x50 bytes at most; a larger count means the bytes are not a layout.
    private const int MaxElements = 64;

    /// <summary>The elements of the layout.</summary>
    public static IReadOnlyList<InputElement> Parse(ReadOnlySpan<byte> meta)
    {
        if (meta.Length < FirstElement)
        {
            throw new InvalidDataException("input layout metadata is shorter than its header");
        }

        var count = BinaryPrimitives.ReadInt32LittleEndian(meta[CountField..]);

        if (count < 0 || count > MaxElements)
        {
            throw new InvalidDataException($"input layout element count {count} is out of range");
        }

        var elements = new List<InputElement>(count);
        var position = FirstElement;

        for (var index = 0; index < count; index++)
        {
            elements.Add(ReadElement(meta, ref position));
        }

        return elements;
    }

    private static InputElement ReadElement(ReadOnlySpan<byte> meta, ref int position)
    {
        var nameLength = BinaryPrimitives.ReadInt32LittleEndian(meta[position..]);
        var nameStart = position + FieldSize;
        var fields = nameStart + nameLength;

        if (nameLength < 1 || fields + (FieldCount * FieldSize) > meta.Length)
        {
            throw new InvalidDataException("input layout element runs past its metadata");
        }

        var name = Encoding.ASCII.GetString(meta.Slice(nameStart, nameLength - 1));
        var values = new Queue<int>(FieldCount);

        for (var index = 0; index < FieldCount; index++)
        {
            values.Enqueue(BinaryPrimitives.ReadInt32LittleEndian(meta[(fields + (index * FieldSize))..]));
        }

        position = fields + (FieldCount * FieldSize);

        return new InputElement(
            name, values.Dequeue(), values.Dequeue(), values.Dequeue(), values.Dequeue(), values.Dequeue(), values.Dequeue());
    }
}
