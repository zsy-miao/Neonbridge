// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Translation.Core;

namespace FiveStar.Shaders.Translation.Waves;

/// <summary>
/// One path of a branch on "any active lane" (s_cbranch_execz / execnz, or a VCC mask that is active and a uniform
/// value), from <c>start</c> to <c>join</c> where the paths meet. A hardware wave without active lanes may take one path
/// while its console wave, with active lanes in other hardware waves, takes the other; both run with this wave's lanes
/// inactive. The path leaves the state at <c>join</c> as it found it (for what is read there) when: EXEC stays empty on
/// those lanes throughout (every EXEC write is an and with an empty mask, an or of empty masks and the like); every
/// write to those lanes is guarded by EXEC; no SGPR, SCC, mask or unguarded VGPR written on the path is read at
/// <c>join</c> before being written (a mask may be when it is empty on those lanes before and after); and the path has
/// no other effect: no s_endpgm, no store, no barrier inside a block. On the path the console runs, GS_ALLOC_REQ is
/// fine (the hardware wave holding the active lanes sends it) and so is a mesh barrier (yield) when GS_ALLOC_REQ came
/// before it and the skipping wave writes no output before its next barrier unless outputs are already open: with no
/// memory shared between lanes (<see cref="WindowAnalysis"/>), a barrier only decides the phase that writes outputs.
/// </summary>
internal sealed class SkipRegion
{
    private readonly WindowGraph _graph;
    private readonly int _start;
    private readonly int _join;
    private readonly WindowState _state;
    private readonly bool _consoleRuns;

    private readonly HashSet<string> _written = new(StringComparer.Ordinal);
    private readonly HashSet<string> _writtenMasks = new(StringComparer.Ordinal);
    private HashSet<string>? _exitZero;
    private bool _yields;

    /// <summary>The path from <paramref name="start"/> to <paramref name="join"/> of a branch taken in <paramref name="state"/>.</summary>
    /// <param name="graph">The program's blocks.</param>
    /// <param name="start">The first block of the path.</param>
    /// <param name="join">Where the branch's paths meet.</param>
    /// <param name="state">The facts at the branch.</param>
    /// <param name="consoleRuns">True for the path the console wave takes, false for one only the hardware wave may take.</param>
    public SkipRegion(WindowGraph graph, int start, int join, WindowState state, bool consoleRuns)
    {
        _graph = graph;
        _start = start;
        _join = join;
        _state = state;
        _consoleRuns = consoleRuns;
    }

    /// <summary>
    /// The window runs the path anyway, inactive (fast waves, WindowAnalysis.EntersAlways): its scalar writes happen as
    /// the console's, so only what it writes per lane counts.
    /// </summary>
    public bool ScalarsRun { get; init; }

    /// <summary>Null when skipping is safe, else why not.</summary>
    public string? Check()
    {
        if (_start == _join)
        {
            return null;
        }

        var initial = new HashSet<string>(_state.Names.Where(name => ValueFact.IsSubset(_state.Get(name).Kind)), StringComparer.Ordinal) { WindowNames.Active };
        var reason = Walk(initial);

        if (reason is not null)
        {
            return reason;
        }

        return After(initial);
    }

    private string? Walk(HashSet<string> initial)
    {
        var entries = new Dictionary<int, (HashSet<string> Zero, bool Allocated)> { [_start] = (initial, _state.Allocated) };
        var pending = new Queue<int>([_start]);

        while (pending.Count > 0)
        {
            var address = pending.Dequeue();
            var block = _graph.Find(address);

            if (block is null)
            {
                return $"the region from {Hlsl.Hex(_start)} ends the program";
            }

            var (zero, allocated) = entries[address];
            zero = [.. zero];
            var reason = Run(block, zero, ref allocated) ?? Leave(block, allocated);

            if (reason is not null)
            {
                return reason;
            }

            foreach (var successor in block.Successors.Where(successor => Enter(entries, successor, zero, allocated)))
            {
                pending.Enqueue(successor);
            }
        }

        return null;
    }

    /// <summary>Records the facts entering a block; true when that block must be visited (again).</summary>
    private bool Enter(Dictionary<int, (HashSet<string> Zero, bool Allocated)> entries, int successor, HashSet<string> zero, bool allocated)
    {
        if (successor == _join)
        {
            _exitZero ??= [.. zero];
            _exitZero.IntersectWith(zero);

            return false;
        }

        if (!entries.TryGetValue(successor, out var known))
        {
            entries[successor] = ([.. zero], allocated);

            return true;
        }

        var narrowed = known.Zero.Where(zero.Contains).ToHashSet(StringComparer.Ordinal);
        var stillAllocated = known.Allocated && allocated;

        if (narrowed.Count == known.Zero.Count && stillAllocated == known.Allocated)
        {
            return false;
        }

        entries[successor] = (narrowed, stillAllocated);

        return true;
    }

    private string? Leave(WindowBlock block, bool allocated)
    {
        if (block.Exit.Kind == BlockExitKind.End || block.Successors.Count == 0)
        {
            return $"the region from {Hlsl.Hex(_start)} ends the program";
        }

        if (block.Exit.Kind != BlockExitKind.Yield)
        {
            return null;
        }

        _yields = true;

        if (!_consoleRuns)
        {
            return $"the path from {Hlsl.Hex(_start)} has a barrier the console wave does not run";
        }

        if (!allocated)
        {
            return $"the region from {Hlsl.Hex(_start)} has a barrier before GS_ALLOC_REQ";
        }

        return null;
    }

    /// <summary>Runs the block's statements on the set of masks empty on the skipping wave's lanes.</summary>
    private string? Run(WindowBlock block, HashSet<string> zero, ref bool allocated)
    {
        foreach (var statement in block.Statements)
        {
            if (statement.IsAllocation && _consoleRuns)
            {
                allocated = true;
                continue;
            }

            if (statement.Kind == StatementKind.Control || (statement.IsGuarded && zero.Contains(WindowNames.Active)))
            {
                continue;
            }

            var reason = Write(statement, zero);

            if (reason is not null)
            {
                return reason;
            }
        }

        return null;
    }

    private string? Write(WindowStatement statement, HashSet<string> zero)
    {
        if (statement.Kind == StatementKind.Effect)
        {
            return $"the region from {Hlsl.Hex(_start)} has an effect: {statement.Text}";
        }

        if (statement.Kind == StatementKind.Assign && statement.IsMask)
        {
            // EXEC may hold the lanes for a moment (a 64-bit EXEC write is two 32-bit halves): a guarded statement then
            // counts as a write to those lanes, and EXEC must be empty there again where the path ends.
            Mark(zero, statement.Target, MaskAlgebra.IsZero(statement.Value, zero));

            return null;
        }

        IReadOnlyList<string> targets = statement.Assigned;

        if (statement.Kind is StatementKind.Assign or StatementKind.Guarded)
        {
            targets = [statement.Target];
        }

        var bitsZero = statement.Kind == StatementKind.Assign && IsZeroInteger(statement.Value, zero);

        foreach (var target in targets)
        {
            Mark(zero, target, bitsZero && !WindowNames.IsMask(target));
        }

        return null;
    }

    /// <summary>An integer lane mask whose bits are zero on the skipping wave's lanes: a ballot of such a mask, or bitwise operations on them.</summary>
    private static bool IsZeroInteger(string value, HashSet<string> zero)
    {
        var ballot = ValueRules.BallotPattern().Match(value);

        if (ballot.Success)
        {
            return zero.Contains(ballot.Groups["mask"].Value);
        }

        var bitwise = ValueRules.BitwisePattern().Match(value);

        if (!bitwise.Success)
        {
            return zero.Contains(value.Trim());
        }

        var (left, right) = (zero.Contains(bitwise.Groups["a"].Value), zero.Contains(bitwise.Groups["b"].Value));

        if (bitwise.Groups["op"].Value == "&")
        {
            return left || right;
        }

        return left && right;
    }

    private void Mark(HashSet<string> zero, string target, bool empty)
    {
        if (!WindowNames.IsMask(target) && !WindowNames.IsTemporary(target) && !(ScalarsRun && WindowNames.IsScalar(target)))
        {
            _written.Add(target);
        }

        if (WindowNames.IsMask(target))
        {
            _writtenMasks.Add(target);
        }

        if (empty)
        {
            zero.Add(target);
        }
        else
        {
            zero.Remove(target);
        }
    }

    /// <summary>The state at <c>join</c>: what the region wrote must not be read there, or must still be the same.</summary>
    private string? After(HashSet<string> initial)
    {
        if (_exitZero is null || !_exitZero.Contains(WindowNames.Active))
        {
            return $"the region from {Hlsl.Hex(_start)} does not end with the lanes inactive";
        }

        var read = _written.FirstOrDefault(name => _graph.IsLiveIn(_join, name));

        if (read is not null)
        {
            return $"the region from {Hlsl.Hex(_start)} writes {read}, read after {Hlsl.Hex(_join)}";
        }

        var mask = _writtenMasks.FirstOrDefault(name => _graph.IsLiveIn(_join, name) && !(initial.Contains(name) && _exitZero.Contains(name)));

        if (mask is not null)
        {
            return $"the region from {Hlsl.Hex(_start)} changes mask {mask}, read after {Hlsl.Hex(_join)}";
        }

        if (_yields && !_state.PhaseTwo && WritesBeforeBarrier(_join))
        {
            return $"the region from {Hlsl.Hex(_start)} has a barrier and {Hlsl.Hex(_join)} exports before the next one";
        }

        return null;
    }

    /// <summary>Whether a path from the block writes a mesh output before a barrier.</summary>
    private bool WritesBeforeBarrier(int start)
    {
        var seen = new HashSet<int>();
        var pending = new Stack<int>([start]);

        while (pending.Count > 0)
        {
            var block = _graph.Find(pending.Pop());

            if (block is null || !seen.Add(block.Address))
            {
                continue;
            }

            if (block.Statements.Any(statement => statement.IsExport))
            {
                return true;
            }

            if (block.Exit.Kind != BlockExitKind.Yield)
            {
                block.Successors.ToList().ForEach(pending.Push);
            }
        }

        return false;
    }
}
