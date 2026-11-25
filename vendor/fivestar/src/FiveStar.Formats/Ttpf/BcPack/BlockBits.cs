// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Buffers.Binary;
using System.Runtime.CompilerServices;

namespace FiveStar.Formats.Ttpf.BcPack;

/// <summary>Writes the fields of one 128-bit block, least significant bit first.</summary>
internal unsafe struct BlockBits
{
    public const int BlockBytes = 16;

    private const int Capacity = 128;

    private UInt128 _value;
    private int _position;

    /// <summary>Appends the low <paramref name="count"/> bits of <paramref name="value"/>; bits past 128 are dropped.</summary>
    [HotPath]
    [MethodImpl(MethodImplOptions.AggressiveInlining)]
    public void Put(ulong value, int count)
    {
        if (count > 0 && _position < Capacity)
        {
            var mask = (1UL << count) - 1;
            _value |= (UInt128)(value & mask) << _position;
        }

        _position += count;
    }

    [HotPath]
    public readonly void WriteTo(byte* output)
    {
        BinaryPrimitives.WriteUInt128LittleEndian(new Span<byte>(output, BlockBytes), _value);
    }
}
