// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Dxil.Ir;

/// <summary>
/// The dominator tree of a function's reachable blocks (Cooper, Harvey and Kennedy, "A Simple, Fast Dominance
/// Algorithm"): immediate dominators by iterating over the reverse postorder until nothing changes.
/// </summary>
public sealed class DominatorTree
{
    private static readonly IReadOnlyList<BasicBlock> None = [];

    private readonly Dictionary<BasicBlock, List<BasicBlock>> _children;

    private DominatorTree(Dictionary<BasicBlock, List<BasicBlock>> children)
    {
        _children = children;
    }

    /// <summary>The blocks <paramref name="block"/> immediately dominates.</summary>
    public IReadOnlyList<BasicBlock> Children(BasicBlock block)
    {
        if (_children.TryGetValue(block, out var children))
        {
            return children;
        }

        return None;
    }

    public static DominatorTree Of(IrFunction function)
    {
        var order = ReversePostorder(function.Blocks[0]);
        var index = new Dictionary<BasicBlock, int>(order.Count);

        for (var position = 0; position < order.Count; position++)
        {
            index[order[position]] = position;
        }

        var predecessors = Predecessors(order, index);
        var dominator = Immediate(order, predecessors);
        var children = new Dictionary<BasicBlock, List<BasicBlock>>();

        for (var position = 1; position < order.Count; position++)
        {
            var parent = order[dominator[position]];

            if (!children.TryGetValue(parent, out var list))
            {
                list = [];
                children[parent] = list;
            }

            list.Add(order[position]);
        }

        return new DominatorTree(children);
    }

    private static int[] Immediate(List<BasicBlock> order, List<int>[] predecessors)
    {
        const int Undefined = -1;
        var dominator = new int[order.Count];
        Array.Fill(dominator, Undefined);
        dominator[0] = 0;
        bool changed = true;

        while (changed)
        {
            changed = false;

            for (var block = 1; block < order.Count; block++)
            {
                var chosen = Undefined;

                foreach (var predecessor in predecessors[block])
                {
                    if (dominator[predecessor] == Undefined)
                    {
                        continue;
                    }

                    chosen = chosen == Undefined ? predecessor : Intersect(dominator, predecessor, chosen);
                }

                if (dominator[block] != chosen)
                {
                    dominator[block] = chosen;
                    changed = true;
                }
            }
        }

        return dominator;
    }

    private static int Intersect(int[] dominator, int left, int right)
    {
        while (left != right)
        {
            while (left > right)
            {
                left = dominator[left];
            }

            while (right > left)
            {
                right = dominator[right];
            }
        }

        return left;
    }

    /// <summary>Predecessor positions of each block in <paramref name="order"/>, reachable predecessors only.</summary>
    private static List<int>[] Predecessors(List<BasicBlock> order, Dictionary<BasicBlock, int> index)
    {
        var predecessors = new List<int>[order.Count];

        for (var position = 0; position < order.Count; position++)
        {
            predecessors[position] = [];
        }

        for (var position = 0; position < order.Count; position++)
        {
            foreach (var successor in order[position].Successors)
            {
                predecessors[index[successor]].Add(position);
            }
        }

        return predecessors;
    }

    private static List<BasicBlock> ReversePostorder(BasicBlock entry)
    {
        var postorder = new List<BasicBlock>();
        var visited = new HashSet<BasicBlock> { entry };
        var stack = new Stack<(BasicBlock Block, IEnumerator<BasicBlock> Successors)>();
        stack.Push((entry, entry.Successors.GetEnumerator()));

        while (stack.Count > 0)
        {
            var (block, successors) = stack.Peek();

            if (successors.MoveNext())
            {
                var next = successors.Current;

                if (visited.Add(next))
                {
                    stack.Push((next, next.Successors.GetEnumerator()));
                }

                continue;
            }

            stack.Pop();
            postorder.Add(block);
        }

        postorder.Reverse();

        return postorder;
    }
}
