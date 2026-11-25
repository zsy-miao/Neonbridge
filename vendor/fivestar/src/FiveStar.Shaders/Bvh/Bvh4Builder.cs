// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Bvh;

/// <summary>
/// Builds a BLAS on the CPU in the node layout the translated shaders read (<see cref="AmdBvhLayout"/>): box32
/// internal nodes with exact f32 bounds, one triangle per type-0 triangle node. For geometry the console builds at run
/// time (dynamic BLASes); the header that places the nodes in a console acceleration structure is the caller's.
/// </summary>
public static class Bvh4Builder
{
    /// <param name="triangles">The triangles, vertices already transformed.</param>
    /// <param name="nodeOffset">Byte offset of the first node from the BVH base; a multiple of 64.</param>
    public static Bvh4Image Build(IReadOnlyList<BvhTriangle> triangles, int nodeOffset)
    {
        CheckAlignment(nodeOffset);

        if (triangles.Count == 0)
        {
            return new Bvh4Image([], AmdBvhLayout.InvalidNode, 0, 0);
        }

        var tree = new Bvh4Tree(triangles.Select(BvhBounds.Of).ToArray());
        tree.Build();

        int boxBytes = tree.Boxes.Count * AmdBvhLayout.BoxNodeSize;
        var nodes = new byte[boxBytes + (tree.Order.Length * AmdBvhLayout.TriangleNodeSize)];
        var leaves = new Bvh4Leaves(boxBytes, AmdBvhLayout.TriangleNodeSize, AmdBvhLayout.TriangleType);
        WriteBoxes(tree, nodes, nodeOffset, leaves);

        for (int position = 0; position < tree.Order.Length; position++)
        {
            var node = nodes.AsSpan(boxBytes + (position * AmdBvhLayout.TriangleNodeSize), AmdBvhLayout.TriangleNodeSize);
            AmdBvhLayout.WriteTriangle(node, triangles[tree.Order[position]]);
        }

        return new Bvh4Image(nodes, Root(nodeOffset), tree.Boxes.Count, tree.Order.Length);
    }

    /// <summary>The root of a tree written at <paramref name="nodeOffset"/>: its first box.</summary>
    internal static uint Root(int nodeOffset)
    {
        return AmdBvhLayout.NodePointer(nodeOffset, AmdBvhLayout.Box32Type);
    }

    internal static void CheckAlignment(int nodeOffset)
    {
        if (nodeOffset % AmdBvhLayout.NodeAlignment != 0)
        {
            throw new ArgumentException($"node offset {nodeOffset} is not 64-byte aligned", nameof(nodeOffset));
        }
    }

    /// <summary>
    /// Writes the box32 nodes of <paramref name="tree"/> at the start of <paramref name="nodes"/>, whose first byte is
    /// <paramref name="nodeOffset"/> bytes from the BVH base; leaf children point into the leaf nodes described by
    /// <paramref name="leaves"/>.
    /// </summary>
    internal static void WriteBoxes(Bvh4Tree tree, Span<byte> nodes, int nodeOffset, Bvh4Leaves leaves)
    {
        Span<uint> pointers = stackalloc uint[AmdBvhLayout.Children];
        Span<BvhBounds> bounds = stackalloc BvhBounds[AmdBvhLayout.Children];

        for (int box = 0; box < tree.Boxes.Count; box++)
        {
            var children = tree.Boxes[box];

            for (int child = 0; child < children.Length; child++)
            {
                pointers[child] = ChildPointer(children[child], nodeOffset, leaves);
                bounds[child] = children[child].Bounds;
            }

            var node = nodes.Slice(box * AmdBvhLayout.BoxNodeSize, AmdBvhLayout.BoxNodeSize);
            AmdBvhLayout.WriteBox32(node, pointers[..children.Length], bounds[..children.Length]);
        }
    }

    private static uint ChildPointer(Bvh4Child child, int nodeOffset, Bvh4Leaves leaves)
    {
        if (child.IsLeaf)
        {
            return AmdBvhLayout.NodePointer(nodeOffset + leaves.Start + (child.Index * leaves.NodeSize), leaves.Type);
        }

        return AmdBvhLayout.NodePointer(nodeOffset + (child.Index * AmdBvhLayout.BoxNodeSize), AmdBvhLayout.Box32Type);
    }
}
