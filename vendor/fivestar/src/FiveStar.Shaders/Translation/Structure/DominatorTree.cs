// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Translation.Structure;

/// <summary>
/// Dominators of a graph from one entry (Cooper, Harvey and Kennedy, "A Simple, Fast Dominance Algorithm"). Built on
/// the reversed graph from an exit node it gives post-dominators. Nodes not reachable from the entry have no order and
/// no dominator.
/// </summary>
internal sealed class DominatorTree
{
    private readonly int?[] _orderIndex;
    private readonly int?[] _dominator;

    public DominatorTree(IReadOnlyList<IReadOnlyList<int>> successors, int entry)
    {
        Entry = entry;
        Order = ReversePostorder(successors, entry);
        _orderIndex = new int?[successors.Count];

        for (var index = 0; index < Order.Count; index++)
        {
            _orderIndex[Order[index]] = index;
        }

        _dominator = Compute(successors);
    }

    public int Entry { get; }

    /// <summary>The reachable nodes in reverse postorder of a depth-first walk from the entry.</summary>
    public IReadOnlyList<int> Order { get; }

    /// <summary>The position of a node in <see cref="Order"/>; null when it is not reachable.</summary>
    public int? OrderIndex(int node)
    {
        return _orderIndex[node];
    }

    /// <summary>The immediate dominator; null for the entry and for unreachable nodes.</summary>
    public int? Immediate(int node)
    {
        if (node == Entry)
        {
            return null;
        }

        return _dominator[node];
    }

    /// <summary>Whether <paramref name="dominator"/> dominates <paramref name="node"/> (every node dominates itself).</summary>
    public bool Dominates(int dominator, int node)
    {
        int? current = node;

        while (current is not null)
        {
            if (current.Value == dominator)
            {
                return true;
            }

            current = Immediate(current.Value);
        }

        return false;
    }

    private static List<int> ReversePostorder(IReadOnlyList<IReadOnlyList<int>> successors, int entry)
    {
        var visited = new bool[successors.Count];
        var postorder = new List<int>();
        var pending = new Stack<(int Node, int Next)>();
        pending.Push((entry, 0));
        visited[entry] = true;

        while (pending.Count > 0)
        {
            var (node, next) = pending.Pop();

            if (next < successors[node].Count)
            {
                pending.Push((node, next + 1));
                var child = successors[node][next];

                if (!visited[child])
                {
                    visited[child] = true;
                    pending.Push((child, 0));
                }

                continue;
            }

            postorder.Add(node);
        }

        postorder.Reverse();

        return postorder;
    }

    private int?[] Compute(IReadOnlyList<IReadOnlyList<int>> successors)
    {
        var predecessors = new List<int>[successors.Count];

        foreach (var node in Order)
        {
            foreach (var successor in successors[node])
            {
                predecessors[successor] ??= [];
                predecessors[successor].Add(node);
            }
        }

        var dominator = new int?[successors.Count];
        dominator[Entry] = Entry;
        var changed = true;

        while (changed)
        {
            changed = false;

            for (var index = 1; index < Order.Count; index++)
            {
                var node = Order[index];
                var candidate = Candidate(predecessors[node], dominator);

                if (candidate != dominator[node])
                {
                    dominator[node] = candidate;
                    changed = true;
                }
            }
        }

        return dominator;
    }

    private int? Candidate(List<int> predecessors, int?[] dominator)
    {
        int? candidate = null;

        foreach (var predecessor in predecessors)
        {
            if (dominator[predecessor] is null)
            {
                continue;
            }

            candidate = candidate is null ? predecessor : Intersect(predecessor, candidate.Value, dominator);
        }

        return candidate;
    }

    private int Intersect(int first, int second, int?[] dominator)
    {
        while (first != second)
        {
            while (_orderIndex[first] > _orderIndex[second])
            {
                first = dominator[first]!.Value;
            }

            while (_orderIndex[second] > _orderIndex[first])
            {
                second = dominator[second]!.Value;
            }
        }

        return first;
    }
}
