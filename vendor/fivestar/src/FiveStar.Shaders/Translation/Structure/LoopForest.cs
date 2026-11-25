// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Translation.Structure;

/// <summary>
/// The natural loops of a graph and how they nest. Every edge that goes back in reverse postorder must lead to a node
/// that dominates its source (a back edge); otherwise the graph is irreducible and cannot be structured. Back edges to
/// the same header form one loop.
/// </summary>
internal sealed class LoopForest
{
    private readonly Dictionary<int, NaturalLoop> _loops = [];
    private readonly Dictionary<int, int> _innermost = [];

    public LoopForest(IReadOnlyList<IReadOnlyList<int>> successors, DominatorTree dominators)
    {
        var sources = BackEdges(successors, dominators);
        var predecessors = Predecessors(successors, dominators);
        var bodies = sources.ToDictionary(pair => pair.Key, pair => Body(pair.Key, pair.Value, predecessors));
        Nest(bodies, successors, dominators);
    }

    /// <summary>The loop with this header.</summary>
    public NaturalLoop Loop(int header)
    {
        return _loops[header];
    }

    public bool IsHeader(int node)
    {
        return _loops.ContainsKey(node);
    }

    /// <summary>The header of the innermost loop holding <paramref name="node"/>; null outside every loop.</summary>
    public int? Innermost(int node)
    {
        if (_innermost.TryGetValue(node, out var header))
        {
            return header;
        }

        return null;
    }

    /// <summary>Back-edge sources per header.</summary>
    private static Dictionary<int, List<int>> BackEdges(IReadOnlyList<IReadOnlyList<int>> successors, DominatorTree dominators)
    {
        var sources = new Dictionary<int, List<int>>();

        foreach (var node in dominators.Order)
        {
            foreach (var target in successors[node])
            {
                if (dominators.OrderIndex(target) > dominators.OrderIndex(node))
                {
                    continue;
                }

                if (!dominators.Dominates(target, node))
                {
                    throw new StructuringFailedException("irreducible");
                }

                if (!sources.TryGetValue(target, out var list))
                {
                    list = [];
                    sources[target] = list;
                }

                list.Add(node);
            }
        }

        return sources;
    }

    private static Dictionary<int, List<int>> Predecessors(IReadOnlyList<IReadOnlyList<int>> successors, DominatorTree dominators)
    {
        var predecessors = new Dictionary<int, List<int>>();

        foreach (var node in dominators.Order)
        {
            foreach (var target in successors[node])
            {
                if (!predecessors.TryGetValue(target, out var list))
                {
                    list = [];
                    predecessors[target] = list;
                }

                list.Add(node);
            }
        }

        return predecessors;
    }

    /// <summary>The header and every node that reaches a back-edge source without passing the header.</summary>
    private static HashSet<int> Body(int header, List<int> sources, Dictionary<int, List<int>> predecessors)
    {
        var body = new HashSet<int> { header };
        var pending = new Stack<int>();

        foreach (var source in sources.Where(body.Add))
        {
            pending.Push(source);
        }

        while (pending.Count > 0)
        {
            var node = pending.Pop();

            foreach (var predecessor in predecessors.GetValueOrDefault(node) ?? [])
            {
                if (body.Add(predecessor))
                {
                    pending.Push(predecessor);
                }
            }
        }

        return body;
    }

    /// <summary>Links each loop to the smallest loop holding its header; larger loops first, so inner ones win.</summary>
    private void Nest(Dictionary<int, HashSet<int>> bodies, IReadOnlyList<IReadOnlyList<int>> successors, DominatorTree dominators)
    {
        var order = bodies.Keys.OrderByDescending(header => bodies[header].Count).ThenBy(header => dominators.OrderIndex(header)).ToList();
        var placed = new List<int>();

        foreach (var header in order)
        {
            var body = bodies[header];
            int? parent = null;

            for (var index = placed.Count - 1; index >= 0 && parent is null; index--)
            {
                if (bodies[placed[index]].Contains(header))
                {
                    parent = placed[index];
                }
            }

            if (parent is not null && !body.IsSubsetOf(bodies[parent.Value]))
            {
                throw new StructuringFailedException("overlapping loops");
            }

            _loops[header] = new NaturalLoop(header, body, parent, Exits(body, successors, dominators));
            placed.Add(header);

            foreach (var node in body)
            {
                _innermost[node] = header;
            }
        }
    }

    private static List<int> Exits(HashSet<int> body, IReadOnlyList<IReadOnlyList<int>> successors, DominatorTree dominators)
    {
        return body.SelectMany(node => successors[node]).Where(target => !body.Contains(target)).Distinct()
            .OrderBy(target => dominators.OrderIndex(target)).ToList();
    }
}
