// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Buffers.Binary;
using System.Numerics;

namespace FiveStar.Shaders.Dxil.Container;

/// <summary>
/// The digest D3D12 checks in a DXBC container (bytes 4-19, over everything from byte 20): MD5 rounds whose last block
/// carries the bit count first and <c>2 × bytes + 1</c> last instead of MD5's padding (DxilContainerHash).
/// </summary>
public static class ContainerHash
{
    private const int BlockBytes = 64;
    private const int ShortTail = 56;
    private const int ShortTailEnd = 60;
    private const int LongTailEnd = 124;
    private const int DigestBytes = 16;
    private const int Rounds = 64;
    private const int RoundsPerStep = 16;
    private const int BitsPerByte = 8;
    private const int StateC = 2;
    private const int StateD = 3;
    private const int StepTwo = 2;
    private const byte PaddingByte = 0x80;
    private const int MultiplierTwo = 5;
    private const int MultiplierThree = 3;
    private const int MultiplierFour = 7;
    private const int OffsetThree = 5;
    private const double SineScale = 4_294_967_296.0;

    private static readonly int[] Shifts = [7, 12, 17, 22, 5, 9, 14, 20, 4, 11, 16, 23, 6, 10, 15, 21];
    private static readonly uint[] Initial = [0x6745_2301, 0xefcd_ab89, 0x98ba_dcfe, 0x1032_5476];
    private static readonly uint[] Sines = [.. Enumerable.Range(1, Rounds).Select(round => (uint)(Math.Abs(Math.Sin(round)) * SineScale))];

    public static byte[] Compute(ReadOnlySpan<byte> data)
    {
        var length = data.Length;
        var left = length % BlockBytes;
        var bitCount = (uint)length * BitsPerByte;
        var tail = new List<byte>();

        if (left < ShortTail)
        {
            tail.AddRange(BitConverter.GetBytes(bitCount));
            tail.AddRange(data[(length - left)..].ToArray());
            tail.Add(PaddingByte);
            Pad(tail, ShortTailEnd);
        }
        else
        {
            tail.AddRange(data[(length - left)..].ToArray());
            tail.Add(PaddingByte);
            Pad(tail, BlockBytes);
            tail.AddRange(BitConverter.GetBytes(bitCount));
            Pad(tail, LongTailEnd);
        }

        tail.AddRange(BitConverter.GetBytes(((uint)length << 1) | 1));
        uint[] state = [.. Initial];

        for (var offset = 0; offset + BlockBytes <= length - left; offset += BlockBytes)
        {
            Block(state, data.Slice(offset, BlockBytes));
        }

        var tailBytes = tail.ToArray();

        for (var offset = 0; offset < tailBytes.Length; offset += BlockBytes)
        {
            Block(state, tailBytes.AsSpan(offset, BlockBytes));
        }

        var digest = new byte[DigestBytes];

        for (var index = 0; index < state.Length; index++)
        {
            BinaryPrimitives.WriteUInt32LittleEndian(digest.AsSpan(index * sizeof(uint)), state[index]);
        }

        return digest;
    }

    private static void Pad(List<byte> bytes, int length)
    {
        while (bytes.Count < length)
        {
            bytes.Add(0);
        }
    }

    private static void Block(uint[] state, ReadOnlySpan<byte> block)
    {
        Span<uint> words = stackalloc uint[RoundsPerStep];

        for (var index = 0; index < RoundsPerStep; index++)
        {
            words[index] = BinaryPrimitives.ReadUInt32LittleEndian(block[(index * sizeof(uint))..]);
        }

        var (a, b, c, d) = (state[0], state[1], state[StateC], state[StateD]);

        for (var round = 0; round < Rounds; round++)
        {
            var (mix, word) = Mix(round, b, c, d);
            var shift = Shifts[((round / RoundsPerStep) * sizeof(uint)) + (round % sizeof(uint))];
            var rotated = BitOperations.RotateLeft(a + mix + Sines[round] + words[word], shift);
            (a, b, c, d) = (d, b + rotated, b, c);
        }

        state[0] += a;
        state[1] += b;
        state[StateC] += c;
        state[StateD] += d;
    }

    private static (uint Mix, int Word) Mix(int round, uint b, uint c, uint d)
    {
        var step = round / RoundsPerStep;

        if (step == 0)
        {
            return ((b & c) | (~b & d), round);
        }

        if (step == 1)
        {
            return ((d & b) | (~d & c), ((MultiplierTwo * round) + 1) % RoundsPerStep);
        }

        if (step == StepTwo)
        {
            return (b ^ c ^ d, ((MultiplierThree * round) + OffsetThree) % RoundsPerStep);
        }

        return (c ^ (b | ~d), (MultiplierFour * round) % RoundsPerStep);
    }
}
