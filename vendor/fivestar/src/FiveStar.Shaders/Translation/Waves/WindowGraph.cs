// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Translation.Core;

namespace FiveStar.Shaders.Translation.Waves;

/// <summary>The translated blocks as a graph, with the variables live on entry to each block.</summary>
internal sealed class WindowGraph
{
    // The node every End block passes control to.
    private const int ProgramEnd = int.MinValue;

    private readonly Dictionary<int, WindowBlock> _blocks = [];
    private readonly Dictionary<int, HashSet<string>> _liveIn = [];

    private Dictionary<int, HashSet<int>>? _postDominators;

    public WindowGraph(IReadOnlyList<CodeBlock> blocks)
    {
        foreach (var block in blocks)
        {
            var statements = block.Lines.SelectMany(line => line.Split('\n')).Select(WindowStatement.Parse).ToList();
            _blocks[block.Address] = new WindowBlock(block.Address, statements, block.Exit, Successors(block.Exit));
        }

        if (blocks.Count > 0)
        {
            Entry = blocks[0].Address;
        }

        ComputeLiveness();
    }

    public int Entry { get; }

    public IEnumerable<WindowBlock> Blocks => _blocks.Values;

    /// <summary>The block at an address, or null (the dispatch loop ends at an address without a block).</summary>
    public WindowBlock? Find(int address)
    {
        return _blocks.GetValueOrDefault(address);
    }

    public bool IsLiveIn(int address, string name)
    {
        return _liveIn.TryGetValue(address, out var live) && live.Contains(name);
    }

    /// <summary>The block where every path from this one meets (its immediate post-dominator), or null when only the end is.</summary>
    public int? Join(int address)
    {
        _postDominators ??= PostDominators();

        if (!_postDominators.TryGetValue(address, out var dominators))
        {
            return null;
        }

        foreach (var candidate in dominators.Where(candidate => candidate != address && candidate != ProgramEnd))
        {
            if (_postDominators[candidate].Count == dominators.Count - 1)
            {
                return candidate;
            }
        }

        return null;
    }

    /// <summary>Per block, the blocks every path from it to the end passes (itself included).</summary>
    private Dictionary<int, HashSet<int>> PostDominators()
    {
        var all = _blocks.Keys.Append(ProgramEnd).ToHashSet();
        var sets = _blocks.Keys.ToDictionary(address => address, _ => new HashSet<int>(all));
        sets[ProgramEnd] = [ProgramEnd];
        var changed = true;

        while (changed)
        {
            changed = false;

            foreach (var block in _blocks.Values.Reverse())
            {
                var next = Exits(block).Select(successor => sets[successor]).Aggregate((first, second) => first.Intersect(second).ToHashSet());
                next = [.. next.Append(block.Address)];

                if (next.Count != sets[block.Address].Count)
                {
                    sets[block.Address] = next;
                    changed = true;
                }
            }
        }

        return sets;
    }

    /// <summary>The successors that are blocks, or the program end when there are none.</summary>
    private IReadOnlyList<int> Exits(WindowBlock block)
    {
        var successors = block.Successors.Where(_blocks.ContainsKey).ToList();

        if (successors.Count < block.Successors.Count)
        {
            successors.Add(ProgramEnd);
        }

        if (successors.Count == 0)
        {
            return [ProgramEnd];
        }

        return successors;
    }

    private static List<int> Successors(BlockExit exit)
    {
        var successors = new List<int>();

        if (exit.Kind != BlockExitKind.End && exit.Target is not null)
        {
            successors.Add((int)exit.Target.Value);
        }

        if (exit.Kind == BlockExitKind.Conditional && exit.Next is not null)
        {
            successors.Add((int)exit.Next.Value);
        }

        return successors;
    }

    private void ComputeLiveness()
    {
        foreach (var block in _blocks.Values)
        {
            _liveIn[block.Address] = new HashSet<string>(StringComparer.Ordinal);
        }

        var changed = true;

        while (changed)
        {
            changed = false;

            foreach (var block in _blocks.Values.Reverse())
            {
                var liveOut = block.Successors.Where(_liveIn.ContainsKey).SelectMany(successor => _liveIn[successor]);
                var live = LiveIn(block, new HashSet<string>(liveOut, StringComparer.Ordinal));

                if (!live.SetEquals(_liveIn[block.Address]))
                {
                    _liveIn[block.Address] = live;
                    changed = true;
                }
            }
        }
    }

    /// <summary>
    /// The variables live on entry, given those live on exit. An assignment to a variable nobody reads afterwards
    /// reads nothing either (a 64-bit EXEC write refreshes <c>active</c> after each half; only the second counts).
    /// </summary>
    private static HashSet<string> LiveIn(WindowBlock block, HashSet<string> live)
    {
        live.UnionWith(block.ConditionReads);

        foreach (var statement in block.Statements.Reverse())
        {
            if (statement.Kind == StatementKind.Assign && !live.Contains(statement.Target))
            {
                continue;
            }

            if (statement.Kind == StatementKind.Assign)
            {
                live.Remove(statement.Target);
            }

            live.UnionWith(statement.Reads);
        }

        return live;
    }
}
