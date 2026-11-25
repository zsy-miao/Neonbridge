// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Tests.Reference;

namespace FiveStar.Shaders.Tests.Cases;

/// <summary>
/// A small handcrafted BVH at <see cref="Base"/> in the GPU-memory window, next to the per-thread query table at 0.
/// Nodes (offsets from the base): 0x000 box32 root (unit box, box at +x, box at +y, big enclosing box); 0x080 box16
/// (f16 boxes, a negative box, an empty slot); 0x0c0 triangle A (quad V0..V3 + V4, node types 0..3 pick the vertex
/// triples, triangle_id 0x24060909); 0x100 triangle B (tilted, I/J swapped); 0x140 triangle C (degenerate); 0x180 only
/// referenced as instance / user node (types 6 / 7: all ones); 0x1c0 box32 with duplicate boxes, a NaN box and a thin slab.
/// </summary>
public static class BvhScene
{
    public const int Base = 0x1000;
    public const int MemorySize = 0x2000;

    public static readonly uint Root = BvhReference.NodePointer(0x000, 5);
    public static readonly uint Box16 = BvhReference.NodePointer(0x080, 4);
    public static readonly uint TriangleA0 = BvhReference.NodePointer(0x0C0, 0);
    public static readonly uint TriangleA1 = BvhReference.NodePointer(0x0C0, 1);
    public static readonly uint TriangleA2 = BvhReference.NodePointer(0x0C0, 2);
    public static readonly uint TriangleA3 = BvhReference.NodePointer(0x0C0, 3);
    public static readonly uint TriangleB = BvhReference.NodePointer(0x100, 0);
    public static readonly uint TriangleC = BvhReference.NodePointer(0x140, 0);
    public static readonly uint Instance = BvhReference.NodePointer(0x180, 6);
    public static readonly uint User = BvhReference.NodePointer(0x180, 7);
    public static readonly uint Duplicates = BvhReference.NodePointer(0x1C0, 5);

    public static IReadOnlyList<uint> BoxNodes => [Root, Box16, Duplicates];

    public static IReadOnlyList<uint> TriangleNodes => [TriangleA0, TriangleA1, TriangleA2, TriangleA3, TriangleB, TriangleC];

    public static BvhMemory Build()
    {
        var memory = new BvhMemory(MemorySize);
        BvhValue nan = BvhValue.Raw(FloatBits.QuietNan);
        memory.WriteBox32(
            Base,
            [TriangleA0, Box16, TriangleA1, 0x12345675],
            [Box([0, 0, 0], [1, 1, 1]), Box([2, 0, 0], [3, 1, 1]), Box([0, 2, 0], [1, 3, 1]), Box([-1, -1, -1], [4, 4, 4])]);
        memory.WriteBox16(
            Base + 0x080,
            [TriangleA0, TriangleB, BvhReference.Invalid, 0x00000007],
            [([0.5, 0.5, 0.5], [1.5, 1.5, 1.5]), ([-2, -2, -2], [-1, -1, -1]), ([0, 0, 0], [0, 0, 0]), ([0.25, 0, 0], [2, 0.75, 3.5])]);
        memory.WriteTriangle(Base + 0x0C0, [[0, 0, 2], [2, 0, 2], [0, 2, 2], [2, 2, 2], [1, 1, 3]], 0x24060909);
        memory.WriteTriangle(Base + 0x100, [[-1, -1, 5], [3, -1, 6], [-1, 3, 4]], 0x06);

        memory.WriteTriangle(Base + 0x140, [[0, 0, 1], [1, 1, 1], [2, 2, 1]], 0x09);
        memory.WriteBox32(
            Base + 0x1C0,
            [TriangleA0, TriangleA1, TriangleB, TriangleC],
            [Box([0, 0, 0], [1, 1, 1]), ([nan, 0, 0], [1, 1, 1]), Box([0, 0, 0], [1, 1, 1]), Box([0.5, -1, -1], [0.5, 2, 2])]);

        return memory;
    }

    private static (BvhValue[] Min, BvhValue[] Max) Box(double[] min, double[] max)
    {
        return ([.. min.Select(BvhValue.Of)], [.. max.Select(BvhValue.Of)]);
    }
}
