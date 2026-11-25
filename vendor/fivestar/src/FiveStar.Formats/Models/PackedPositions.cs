// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Buffers.Binary;

namespace FiveStar.Formats.Models;

/// <summary>
/// Positions packed into 8 bytes as three unsigned fields: A = bits 8-31 of word 0 (24 bits), B = bits 0-7 of word 0
/// plus bits 0-15 of word 1 as its high part (24 bits), C = bits 16-31 of word 1 (16 bits). The 32 bytes before the
/// stream hold, per field, a float minimum and a float step (value = minimum + field * step), and which axis field C
/// holds; A and B take the other two axes in a fixed order.
/// </summary>
public static class PackedPositions
{
    /// <summary>Bytes from the header to the stream: min[3], axis of C, step[3], one more float (4.0 so far).</summary>
    public const int HeaderBytes = 0x20;

    public const int Stride = 8;

    private const int StepOffset = 0x10;
    private const int AxisOffset = 0x0C;
    private const int Fields = 3;
    private const int FieldA = 0;
    private const int FieldB = 1;
    private const int FieldC = 2;
    private const int FieldAShift = 8;
    private const int FieldBLowMask = 0xFF;
    private const int FieldBHighMask = 0xFFFF;
    private const int FieldBHighShift = 8;
    private const int FieldCShift = 16;
    private const int X = 0;
    private const int Y = 1;
    private const int Z = 2;

    // The axis that fields A, B and C hold, by the header's axis-of-C word (checked against the stored normals).
    private static readonly int[][] FieldAxes = [[Z, Y, X], [Z, X, Y], [X, Y, Z]];

    /// <summary>Decodes <paramref name="count"/> positions as x, y, z triples.</summary>
    public static float[] Decode(ReadOnlySpan<byte> header, ReadOnlySpan<byte> stream, int count)
    {
        var axisWord = BinaryPrimitives.ReadUInt32LittleEndian(header[AxisOffset..]);

        if (axisWord >= FieldAxes.Length)
        {
            throw new ModelFormatException($"position header names axis {axisWord}");
        }

        var axes = FieldAxes[axisWord];
        var minimum = new float[Fields];
        var step = new float[Fields];

        for (var field = 0; field < Fields; field++)
        {
            minimum[field] = BinaryPrimitives.ReadSingleLittleEndian(header[(field * sizeof(float))..]);
            step[field] = BinaryPrimitives.ReadSingleLittleEndian(header[(StepOffset + (field * sizeof(float)))..]);
        }

        var positions = new float[count * Fields];
        Span<uint> values = stackalloc uint[Fields];

        for (var vertex = 0; vertex < count; vertex++)
        {
            var low = BinaryPrimitives.ReadUInt32LittleEndian(stream[(vertex * Stride)..]);
            var high = BinaryPrimitives.ReadUInt32LittleEndian(stream[((vertex * Stride) + sizeof(uint))..]);
            values[FieldA] = low >> FieldAShift;
            values[FieldB] = (low & FieldBLowMask) | ((high & FieldBHighMask) << FieldBHighShift);
            values[FieldC] = high >> FieldCShift;

            for (var field = 0; field < Fields; field++)
            {
                positions[(vertex * Fields) + axes[field]] = minimum[field] + (values[field] * step[field]);
            }
        }

        return positions;
    }
}
