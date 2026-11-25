// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Text.RegularExpressions;
using FiveStar.Shaders.Translation.Core;

namespace FiveStar.Shaders.Translation.Waves;

/// <summary>
/// Whether a translated mesh program gives console results on a hardware wave narrower than its console wave, where the
/// hardware wave runs a window of consecutive console lanes and the other lanes of the console wave are live threads of
/// other hardware waves. A forward pass over the blocks tracks which values are the console's (<see cref="ValueFact"/>):
/// wave reductions (<c>vw_any</c>, <c>vw_count</c>, <c>vw_ballot</c>) see only the window, so their results are not,
/// unless they are never read; <c>readfirstlane</c> is when its value is equal on all active lanes and the window holds
/// one. Every branch, output, store and message must read only console values, except a branch on "any active lane"
/// that a window without active lanes may take the other way than its console wave: that is fine when both paths, run
/// with the window's lanes inactive, change nothing the window reads after they meet (<see cref="SkipRegion"/>). Lanes that share LDS or memory without a
/// barrier between them are not handled: those windows would need to run in step.
/// </summary>
internal sealed partial class WindowAnalysis
{
    private const int StepsPerBlock = 64;

    private readonly WindowGraph _graph;
    private readonly IReadOnlyList<string> _prologue;
    private readonly int _threads;
    private readonly Dictionary<int, WindowState> _entries = [];
    private readonly Dictionary<int, int> _entersAlways = [];

    public WindowAnalysis(IReadOnlyList<CodeBlock> blocks, IReadOnlyList<string> prologue, int threads)
    {
        _graph = new WindowGraph(blocks);
        _prologue = prologue;
        _threads = threads;
    }

    /// <summary>
    /// Fast waves (TranslationOptions.FastWaves): each window's <c>readfirstlane</c> is taken for the console's, a branch
    /// on "any lane" of a mask or a ballot is the window's own (its skipped path checked as for a window without active
    /// lanes), or a skip every window takes (<see cref="EnteredAlways"/>).
    /// </summary>
    public bool FastWaves { get; init; }

    /// <summary>Fast waves: the blocks whose skip every window takes (<see cref="EntersAlways"/>), with the path it enters.</summary>
    public IReadOnlyDictionary<int, int> EnteredAlways => _entersAlways;

    /// <summary>Null when every window gives console results, else the first reason found why not.</summary>
    public string? Check()
    {
        var shared = _graph.Blocks.SelectMany(block => block.Statements).FirstOrDefault(statement => statement.IsShared);

        if (shared is not null)
        {
            return $"lanes share memory: {shared.Text}";
        }

        var read = _graph.Blocks.SelectMany(block => block.Statements).FirstOrDefault(statement => FastWaves && statement.Calls.Any(IsLaneRead));

        if (read is not null)
        {
            return $"reads a lane by number: {read.Text}";
        }

        var start = new WindowState { FastWaves = FastWaves };
        var transfer = new StatementTransfer(start, _threads);

        foreach (var line in _prologue)
        {
            transfer.Apply(WindowStatement.Parse(line));
        }

        return Solve(start);
    }

    private string? Solve(WindowState start)
    {
        _entries[_graph.Entry] = start;
        var pending = new Queue<int>([_graph.Entry]);
        var limit = StepsPerBlock * (_graph.Blocks.Count() + 1);

        for (var step = 0; pending.Count > 0; step++)
        {
            if (step > limit)
            {
                return "the analysis did not settle";
            }

            var block = _graph.Find(pending.Dequeue());

            if (block is null)
            {
                continue;
            }

            var reason = Visit(block, pending);

            if (reason is not null)
            {
                return $"{Hlsl.Hex(block.Address)}: {reason}";
            }
        }

        return null;
    }

    private string? Visit(WindowBlock block, Queue<int> pending)
    {
        var state = _entries[block.Address].Clone();
        var transfer = new StatementTransfer(state, _threads);

        foreach (var statement in block.Statements)
        {
            var reason = transfer.Apply(statement);

            if (reason is not null)
            {
                return reason;
            }
        }

        var exit = block.Exit;

        if (exit.Kind == BlockExitKind.Yield)
        {
            state.PhaseTwo |= state.Allocated;
        }

        if (exit.Kind != BlockExitKind.Conditional)
        {
            block.Successors.ToList().ForEach(successor => Propagate(successor, state, pending));

            return null;
        }

        return Branch(block, state, pending);
    }

    private string? Branch(WindowBlock block, WindowState state, Queue<int> pending)
    {
        var exit = block.Exit;
        var condition = exit.Condition ?? string.Empty;
        var (target, next) = ((int)exit.Target!.Value, (int)exit.Next!.Value);

        var fact = ValueRules.Condition(condition, state);

        // Fast waves: a window may decide "any lane" for itself, but the path it skips must change nothing it reads after.
        var fast = FastAnyPattern().Match(condition);
        var skipChecked = state.FastWaves && fast.Success && !ValueRules.Condition(condition, state.Strict()).IsExact;
        _entersAlways.Remove(block.Address);

        if (fact.IsExact && !skipChecked)
        {
            Propagate(target, state, pending);
            Propagate(next, state, pending);

            return null;
        }

        var any = AnyPattern().Match(condition);

        if (!skipChecked && (!any.Success || !IsActiveTest(any.Groups["mask"].Value, state)))
        {
            return HeldBranch(block, state, pending, fact.IsHeld);
        }

        return Skip(block, state, pending, fast, skipChecked);
    }

    /// <summary>
    /// A branch on "any active lane": a window without one may skip the path its console wave runs, so both paths, run
    /// with its lanes inactive, must change nothing it reads after they meet; with fast waves every window may take the
    /// path instead (<see cref="EntersAlways"/>).
    /// </summary>
    private string? Skip(WindowBlock block, WindowState state, Queue<int> pending, Match any, bool fast)
    {
        var exit = block.Exit;
        var (some, none) = ((int)exit.Target!.Value, (int)exit.Next!.Value);

        if (any.Groups["not"].Value.Length > 0)
        {
            (some, none) = (none, some);
        }

        var join = _graph.Join(block.Address);

        if (join is null)
        {
            return $"branch on {exit.Condition}: its paths meet only at the end";
        }

        var reason = new SkipRegion(_graph, some, join.Value, state, true).Check() ?? new SkipRegion(_graph, none, join.Value, state, false).Check();

        if (reason is not null && fast && EntersAlways(block, any.Groups["mask"].Value, some, join.Value, state))
        {
            Propagate(some, state, pending);

            return null;
        }

        if (reason is not null)
        {
            return reason;
        }

        var holding = state.Clone();
        holding.HoldsActiveLane();
        Propagate(none, state, pending);
        Propagate(some, holding, pending);

        return null;
    }

    /// <summary>
    /// Fast waves: a skip whose path a window without active lanes cannot leave out (it writes scalars read after the
    /// join) is taken by every window, as the console takes it whenever another of its lanes is active. That needs the
    /// mask to be active (and a uniform value), so lanes outside it run the path inactive, and the path to pass for
    /// such a lane but for its scalars. Exact but when no lane of the console wave is active: the console then skips.
    /// </summary>
    private bool EntersAlways(WindowBlock block, string mask, int some, int join, WindowState state)
    {
        if (mask.Length == 0 || (mask != WindowNames.Active && state.Get(mask).Kind != MaskKind.ActiveAnd) || Returns(some, block.Address, join))
        {
            return false;
        }

        if (new SkipRegion(_graph, some, join, state, true) { ScalarsRun = true }.Check() is not null)
        {
            return false;
        }

        _entersAlways[block.Address] = some;

        return true;
    }

    /// <summary>
    /// Whether the path from <paramref name="start"/> comes back to <paramref name="branch"/> before <paramref name="join"/>:
    /// the branch then tests a loop's exit, and a jump in its place would never leave the loop.
    /// </summary>
    private bool Returns(int start, int branch, int join)
    {
        var seen = new HashSet<int>();
        var pending = new Stack<int>([start]);

        while (pending.Count > 0)
        {
            var address = pending.Pop();

            if (address == branch)
            {
                return true;
            }

            if (address == join || !seen.Add(address))
            {
                continue;
            }

            _graph.Find(address)?.Successors.ToList().ForEach(pending.Push);
        }

        return false;
    }

    /// <summary>
    /// A condition that is the console's when the window holds an active lane: a window without one may take either
    /// path, so both must leave its state as they found it, and neither may hold what only the console runs.
    /// </summary>
    private string? HeldBranch(WindowBlock block, WindowState state, Queue<int> pending, bool held)
    {
        var exit = block.Exit;
        var condition = exit.Condition ?? string.Empty;
        var join = _graph.Join(block.Address);

        if (!held || !state.Get(WindowNames.Active).IsExact || join is null)
        {
            return $"branch on {condition}";
        }

        var (target, next) = ((int)exit.Target!.Value, (int)exit.Next!.Value);
        var reason = new SkipRegion(_graph, target, join.Value, state, false).Check() ?? new SkipRegion(_graph, next, join.Value, state, false).Check();

        if (reason is not null)
        {
            return $"branch on {condition}: {reason}";
        }

        Propagate(target, state, pending);
        Propagate(next, state, pending);

        return null;
    }

    /// <summary>The mask is EXEC, or EXEC and a uniform console value: "any lane of it" means "any active lane".</summary>
    private static bool IsActiveTest(string mask, WindowState state)
    {
        var fact = state.Get(mask);
        var active = state.Get(WindowNames.Active);

        return active.IsExact && fact.IsHeld && (mask == WindowNames.Active || fact.Kind == MaskKind.ActiveAnd);
    }

    private void Propagate(int successor, WindowState state, Queue<int> pending)
    {
        if (!_entries.TryGetValue(successor, out var known))
        {
            _entries[successor] = state.Clone();
            pending.Enqueue(successor);

            return;
        }

        if (known.MergeFrom(state))
        {
            pending.Enqueue(successor);
        }
    }

    /// <summary>A call fast waves cannot take per window: another lane by number, a count, or a native wave operation.</summary>
    private static bool IsLaneRead(string call)
    {
        return call is "vw_read" or "vw_read_fill" or "vw_count" or "vw_origin" || (call.StartsWith("Wave", StringComparison.Ordinal) && call != "WaveIsFirstLane");
    }

    [GeneratedRegex(@"^(?<not>!?)vw_any\((?<mask>[A-Za-z_]\w*)\)$")]
    private static partial Regex AnyPattern();

    // "Any lane" of a mask, or of a ballot's two halves, as fast waves see it.
    [GeneratedRegex(@"^(?<not>!?)(?:vw_any\(\(?(?<mask>[A-Za-z_]\w*)\)?\)|\(?\([A-Za-z_]\w* \| [A-Za-z_]\w*\) != 0\)?)$")]
    private static partial Regex FastAnyPattern();
}
