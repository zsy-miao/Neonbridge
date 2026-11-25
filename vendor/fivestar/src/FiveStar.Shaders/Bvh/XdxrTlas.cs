// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Bvh;

/// <summary>
/// A TLAS built on the CPU in the console's layout: the 64-byte header (root node pointer at 0x30, where the traversal
/// reads it), box32 nodes over the instances' world bounds, then one <see cref="XdxrInstanceNode"/> per instance.
/// Instances without a BLAS (null address or no root) or with a singular transform are left out, as DXR treats a null
/// BLAS as inactive. Node pointers count from the start of the structure.
/// </summary>
public static class XdxrTlas
{
    /// <summary>Dword 0 of a TLAS: format version 6, bit 31 (bottom level) clear.</summary>
    private const uint TopLevelFlags = 0x6;

    /// <summary>Builds the structure over <paramref name="instances"/>; each node records its index in the list.</summary>
    public static byte[] Build(IReadOnlyList<BvhInstance> instances)
    {
        var active = new List<int>();
        var bounds = new List<BvhBounds>();

        for (int i = 0; i < instances.Count; i++)
        {
            var instance = instances[i];

            if (instance.BlasAddress == 0 || instance.BlasRoot == AmdBvhLayout.InvalidNode)
            {
                continue;
            }

            active.Add(i);
            bounds.Add(XdxrInstanceNode.WorldBounds(instance));
        }

        if (active.Count == 0)
        {
            var empty = new byte[XdxrBlas.HeaderSize];
            XdxrBlas.WriteCommon(empty, TopLevelFlags, 0, BvhBounds.Empty, AmdBvhLayout.InvalidNode, XdxrBlas.HeaderSize);

            return empty;
        }

        return Write(instances, active, bounds);
    }

    private static byte[] Write(IReadOnlyList<BvhInstance> instances, List<int> active, List<BvhBounds> bounds)
    {
        var tree = new Bvh4Tree(bounds);
        tree.Build();

        int boxBytes = tree.Boxes.Count * AmdBvhLayout.BoxNodeSize;
        int leaves = XdxrBlas.HeaderSize + boxBytes;
        var bytes = new byte[leaves + (active.Count * XdxrInstanceNode.Size)];
        var layout = new Bvh4Leaves(boxBytes, XdxrInstanceNode.Size, XdxrInstanceNode.Type);
        Bvh4Builder.WriteBoxes(tree, bytes.AsSpan(XdxrBlas.HeaderSize), XdxrBlas.HeaderSize, layout);
        var all = BvhBounds.Empty;

        for (int position = 0; position < tree.Order.Length; position++)
        {
            int leaf = tree.Order[position];
            int index = active[leaf];
            var node = bytes.AsSpan(leaves + (position * XdxrInstanceNode.Size), XdxrInstanceNode.Size);

            // A singular transform leaves the node zero: mask 0, which no ray's mask selects.
            XdxrInstanceNode.Write(node, instances[index], (uint)index);
            all = all.Union(bounds[leaf]);
        }

        XdxrBlas.WriteCommon(bytes, TopLevelFlags, active.Count, all, Bvh4Builder.Root(XdxrBlas.HeaderSize), leaves);

        return bytes;
    }
}
