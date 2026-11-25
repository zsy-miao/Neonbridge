// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Translation.Core;

namespace FiveStar.Shaders.Translation.Structure;

/// <summary>
/// The control-flow graph of the translated blocks. Nodes 0..n-1 are the blocks in address order; node n is the
/// program end, reached by s_endpgm, a mesh yield, the fall-through past the last block and a branch to an address that
/// starts no block (the dispatch form's <c>default</c> case). A conditional block lists its taken target first.
/// </summary>
internal sealed class FlowGraph
{
    private readonly int[][] _successors;
    private readonly Dictionary<long, int> _nodes = [];

    public FlowGraph(IReadOnlyList<CodeBlock> blocks)
    {
        Blocks = blocks;
        End = blocks.Count;

        for (var node = 0; node < blocks.Count; node++)
        {
            _nodes[blocks[node].Address] = node;
        }

        _successors = new int[blocks.Count + 1][];
        _successors[End] = [];

        for (var node = 0; node < blocks.Count; node++)
        {
            _successors[node] = Targets(blocks[node].Exit);
        }
    }

    public IReadOnlyList<CodeBlock> Blocks { get; }

    /// <summary>The program-end node.</summary>
    public int End { get; }

    /// <summary>The number of nodes, the end node included.</summary>
    public int Count => End + 1;

    /// <summary>The successors in the order of <see cref="Targets"/>; the end node has none.</summary>
    public IReadOnlyList<IReadOnlyList<int>> Successors => _successors;

    /// <summary>The block starting at <paramref name="address"/>, if any.</summary>
    public int? NodeAt(long address)
    {
        if (_nodes.TryGetValue(address, out var node))
        {
            return node;
        }

        return null;
    }

    private int[] Targets(BlockExit exit)
    {
        switch (exit.Kind)
        {
            case BlockExitKind.Jump:
                return [Node(exit.Target)];
            case BlockExitKind.Conditional:
                return [Node(exit.Target), Node(exit.Next)];
            default:
                return [End];
        }
    }

    private int Node(long? address)
    {
        if (address is null)
        {
            return End;
        }

        return NodeAt(address.Value) ?? End;
    }
}
