// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Buffers.Binary;

namespace FiveStar.Shaders.Tests.Reference;

/// <summary>
/// Little-endian memory holding a BVH in the GPU-memory window, with writers for the GPURT gfx10 node layouts:
/// box32 (4 child pointers, 4 x (min3, max3) f32), box16 (4 child pointers, 4 x 3 dwords of packed f16) and triangle
/// nodes (vertices V0..V3 at 0/12/24/36, V4 at 48, triangle_id at 60).
/// </summary>
public sealed class BvhMemory
{
    private readonly byte[] _bytes;

    public BvhMemory(int size)
    {
        _bytes = new byte[size];
    }

    public void WriteWord(int address, uint value)
    {
        BinaryPrimitives.WriteUInt32LittleEndian(_bytes.AsSpan(address), value);
    }

    public uint ReadWord(int address)
    {
        return BinaryPrimitives.ReadUInt32LittleEndian(_bytes.AsSpan(address));
    }

    public double ReadFloat(int address)
    {
        return FloatBits.Value(ReadWord(address));
    }

    /// <summary>A float slot: a number, or raw bits (NaN payloads) when given as <see cref="BvhValue.Raw"/>.</summary>
    public void WriteFloat(int address, BvhValue value)
    {
        WriteWord(address, value.Bits);
    }

    public void WriteBox32(int address, uint[] children, (BvhValue[] Min, BvhValue[] Max)[] boxes)
    {
        WriteChildren(address, children);

        for (var box = 0; box < boxes.Length; box++)
        {
            for (var axis = 0; axis < 3; axis++)
            {
                WriteFloat(address + 16 + (24 * box) + (4 * axis), boxes[box].Min[axis]);
                WriteFloat(address + 28 + (24 * box) + (4 * axis), boxes[box].Max[axis]);
            }
        }
    }

    /// <summary>f16 boxes: per child (minx | miny &lt;&lt; 16, minz | maxx &lt;&lt; 16, maxy | maxz &lt;&lt; 16).</summary>
    public void WriteBox16(int address, uint[] children, (double[] Min, double[] Max)[] boxes)
    {
        WriteChildren(address, children);

        for (var box = 0; box < boxes.Length; box++)
        {
            var (min, max) = boxes[box];
            var at = address + 16 + (12 * box);
            WriteWord(at, FloatBits.Pack(FloatBits.Half(min[0]), FloatBits.Half(min[1])));
            WriteWord(at + 4, FloatBits.Pack(FloatBits.Half(min[2]), FloatBits.Half(max[0])));
            WriteWord(at + 8, FloatBits.Pack(FloatBits.Half(max[1]), FloatBits.Half(max[2])));
        }
    }

    public void WriteTriangle(int address, double[][] vertices, uint triangleId)
    {
        for (var vertex = 0; vertex < vertices.Length; vertex++)
        {
            for (var axis = 0; axis < 3; axis++)
            {
                WriteFloat(address + (12 * vertex) + (4 * axis), BvhValue.Of(vertices[vertex][axis]));
            }
        }

        WriteWord(address + 60, triangleId);
    }

    public uint[] Words()
    {
        var words = new uint[_bytes.Length / 4];

        for (var index = 0; index < words.Length; index++)
        {
            words[index] = ReadWord(4 * index);
        }

        return words;
    }

    private void WriteChildren(int address, uint[] children)
    {
        for (var child = 0; child < children.Length; child++)
        {
            WriteWord(address + (4 * child), children[child]);
        }
    }
}
