// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Buffers.Binary;
using FiveStar.Formats.Common;

namespace FiveStar.Formats.Pso;

/// <summary>Reads the fixed-layout scalar, vector and matrix members of a PSO into tree values.</summary>
internal static class PsoScalars
{
    private const int Vec2Floats = 2;
    private const int Vec3Floats = 3;
    private const int Vec4Floats = 4;
    private const int Mat34Floats = 12;
    private const int Mat44Floats = 16;

    /// <summary>The value of a scalar member, or <c>Found = false</c> when the code is not a scalar.</summary>
    public static (bool Found, object? Value) TryRead(byte[] data, long at, PsoTypeCode code, int vectorDecimals)
    {
        switch (code)
        {
            case PsoTypeCode.Bool:
                return (true, Bytes(data, at, sizeof(byte))[0] != 0);
            case PsoTypeCode.S8:
                return (true, (long)(sbyte)Bytes(data, at, sizeof(sbyte))[0]);
            case PsoTypeCode.U8:
                return (true, (long)Bytes(data, at, sizeof(byte))[0]);
            case PsoTypeCode.S16:
                return (true, (long)BinaryPrimitives.ReadInt16LittleEndian(Bytes(data, at, sizeof(short))));
            case PsoTypeCode.U16:
                return (true, (long)BinaryPrimitives.ReadUInt16LittleEndian(Bytes(data, at, sizeof(ushort))));
            case PsoTypeCode.S32:
                return (true, (long)BinaryPrimitives.ReadInt32LittleEndian(Bytes(data, at, sizeof(int))));
            case PsoTypeCode.U32:
                return (true, (long)BinaryPrimitives.ReadUInt32LittleEndian(Bytes(data, at, sizeof(uint))));
            case PsoTypeCode.U64:
                return (true, BinaryPrimitives.ReadUInt64LittleEndian(Bytes(data, at, sizeof(ulong))));
            case PsoTypeCode.S64:
                return (true, BinaryPrimitives.ReadInt64LittleEndian(Bytes(data, at, sizeof(long))));
            default:
                return TryReadFloat(data, at, code, vectorDecimals);
        }
    }

    private static (bool Found, object? Value) TryReadFloat(byte[] data, long at, PsoTypeCode code, int decimals)
    {
        switch (code)
        {
            case PsoTypeCode.F16:
                return (true, (double)BinaryPrimitives.ReadHalfLittleEndian(Bytes(data, at, sizeof(ushort))));
            case PsoTypeCode.F32:
                return (true, (double)BinaryPrimitives.ReadSingleLittleEndian(Bytes(data, at, sizeof(float))));
            case PsoTypeCode.F64:
                return (true, BinaryPrimitives.ReadDoubleLittleEndian(Bytes(data, at, sizeof(double))));
            case PsoTypeCode.Vec2:
                return (true, Floats(data, at, Vec2Floats, decimals));
            case PsoTypeCode.Vec3:
            case PsoTypeCode.Vec3Packed:
                return (true, Floats(data, at, Vec3Floats, decimals));
            case PsoTypeCode.Vec4:
            case PsoTypeCode.Vec3V:
                return (true, Floats(data, at, Vec4Floats, decimals));
            case PsoTypeCode.Mat34:
                return (true, Floats(data, at, Mat34Floats, decimals));
            case PsoTypeCode.Mat44:
                return (true, Floats(data, at, Mat44Floats, decimals));
            default:
                return (false, null);
        }
    }

    private static List<object?> Floats(byte[] data, long at, int count, int decimals)
    {
        var bytes = Bytes(data, at, count * sizeof(float));
        var values = new List<object?>(count);

        for (var index = 0; index < count; index++)
        {
            var value = BinaryPrimitives.ReadSingleLittleEndian(bytes[(index * sizeof(float))..]);
            values.Add(DecimalText.Round(value, decimals));
        }

        return values;
    }

    private static ReadOnlySpan<byte> Bytes(byte[] data, long at, int size)
    {
        PsoBytes.Check(data, at, size);

        return data.AsSpan((int)at, size);
    }
}
