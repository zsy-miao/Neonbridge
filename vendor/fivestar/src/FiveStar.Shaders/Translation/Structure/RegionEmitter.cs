// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Translation.Core;

namespace FiveStar.Shaders.Translation.Structure;

/// <summary>
/// Emits the blocks reachable from one entry as structured HLSL. A branching block becomes <c>if</c>/<c>else</c> whose
/// arms run up to the join of <see cref="ScopeGraph.Join"/>; when the arms never meet, the smaller ones go inside the
/// <c>if</c> and end by leaving the scope, and the largest follows it. A natural loop becomes <c>[loop] for (;;)</c>: a
/// branch back to the header is <c>continue</c> (nothing at the end of the body), a branch out is <c>break</c>, and a loop
/// with several exits records which one it took in a selector that the code after it tests. An early end of the region
/// outside every loop is a <c>break</c> out of a <c>do { } while (false)</c> around the region. A block reached on paths
/// that do not join before it is emitted once per path. When that copying makes a scope (the region's top level or a
/// loop body) larger than its limit, the scope is emitted guarded instead: its blocks in topological order, each under
/// <c>if (flowN == k)</c>, each setting <c>flowN</c> to its successor; nested loops stay structured. Every branch condition
/// is wave-uniform, so all lanes take the same arm, exactly as with the dispatch loop. Every loop iteration spends one of
/// the invocation's <c>loop_budget</c> (<see cref="LoopGuard"/>); once it is spent, a loop leaves through its first exit.
/// </summary>
internal sealed class RegionEmitter
{
    private const int IndentWidth = 2;
    private const string StatementIndent = "  ";

    // dxc (clang) allows 256 nested brackets in total; block statements use some of them.
    private const int MaxDepth = 120;

    // A scope's emitted block lines may exceed its own by this percentage plus a few blocks' worth (copied blocks).
    private const int GrowthPercent = 150;
    private const int GrowthSlack = 256;
    private const int Percent = 100;

    private readonly FlowGraph _graph;
    private readonly LoopForest _forest;
    private readonly ScopeGraph _top;
    private readonly Func<string, bool> _keep;
    private readonly string _cap;
    private readonly Dictionary<int, ScopeGraph> _scopes = [];
    private readonly List<string> _lines = [];
    private readonly List<ScopeFrame> _frames = [];
    private int _emitted;
    private int _loops;
    private int _loopGuards;
    private int _guards;
    private int _frameCount;
    private bool _earlyEnd;

    /// <summary>
    /// A region from <paramref name="entry"/>; lines failing <paramref name="keep"/> are left out, and loops stop once
    /// <c>loop_budget</c> passes <paramref name="cap"/> (an HLSL literal).
    /// </summary>
    public RegionEmitter(FlowGraph graph, int entry, Func<string, bool> keep, string cap)
    {
        _graph = graph;
        _keep = keep;
        _cap = cap;
        var dominators = new DominatorTree(graph.Successors, entry);
        _forest = new LoopForest(graph.Successors, dominators);
        _top = new ScopeGraph(graph, _forest, null, entry);
    }

    /// <summary>The region's statements, nested <paramref name="depth"/> levels below the function body.</summary>
    public List<string> Emit(int depth)
    {
        EmitScope(_top, null, depth);

        if (_earlyEnd)
        {
            _lines.Insert(0, StatementIndent + Indentation(depth) + "do {");
            Add(depth, "} while (false);");
        }

        return _lines;
    }

    /// <summary>Emits a whole scope structured, or guarded when the structured code grows too large.</summary>
    private void EmitScope(ScopeGraph scope, string? exitSelector, int depth)
    {
        var mark = (Lines: _lines.Count, Emitted: _emitted, EarlyEnd: _earlyEnd);
        var limit = (scope.TotalWeight() * GrowthPercent / Percent) + GrowthSlack;
        var frame = new ScopeFrame(_frameCount, _emitted, limit, Guarded: false);
        _frameCount++;
        _frames.Add(frame);

        try
        {
            EmitFrom(new Flow(scope, exitSelector, scope.Sink, Tail: true), scope.Entry, depth);
        }
        catch (ScopeOverflowException exception) when (exception.Frame == frame.Id)
        {
            _lines.RemoveRange(mark.Lines, _lines.Count - mark.Lines);
            _emitted = mark.Emitted;
            _earlyEnd = mark.EarlyEnd;
            _frames[^1] = frame with { Guarded = true };
            EmitGuarded(scope, exitSelector, depth);
        }
        finally
        {
            _frames.RemoveAt(_frames.Count - 1);
        }
    }

    /// <summary>Emits from <paramref name="start"/> up to (not including) the flow's stop node.</summary>
    private void EmitFrom(Flow flow, int start, int depth)
    {
        if (depth > MaxDepth)
        {
            Overflow("nesting");
        }

        var scope = flow.Scope;
        var local = start;

        while (local != flow.Stop)
        {
            var node = scope.Node(local);

            if (node.Kind is not ScopeNodeKind.Block and not ScopeNodeKind.Loop)
            {
                EmitLeave(flow, node, depth);

                return;
            }

            var loopSelector = EmitNode(node, depth);
            var successors = scope.Successors(local);

            if (successors.Distinct().Count() == 1)
            {
                local = successors[0];

                continue;
            }

            var join = Join(flow, local);
            var choice = new Choice(node, loopSelector, successors, join ?? scope.Sink);
            local = join is null ? EmitSplit(flow, choice, depth) : EmitBranches(flow, choice, depth);
        }
    }

    /// <summary>
    /// The join of a choice inside a run that stops at another node: when an arm reaches the stop, a join that does not
    /// lead to the stop would copy the code from the stop on into the arms, so the arms run up to the stop instead.
    /// </summary>
    private static int? Join(Flow flow, int local)
    {
        var scope = flow.Scope;
        var join = scope.Join(local);

        if (flow.Stop == scope.Sink || join == flow.Stop)
        {
            return join;
        }

        if (join is not null && scope.Reaches(join.Value, flow.Stop))
        {
            return join;
        }

        if (scope.Successors(local).Any(target => scope.Reaches(target, flow.Stop)))
        {
            return flow.Stop;
        }

        return join;
    }

    /// <summary>
    /// Leaves the scope: <c>continue</c> for the loop header, <c>break</c> (after setting the selector) for a loop exit,
    /// and at the top level a <c>break</c> out of the region. At the end of a loop body or of the region nothing is needed.
    /// </summary>
    private void EmitLeave(Flow flow, ScopeNode node, int depth)
    {
        var loop = flow.Scope.Loop;

        if (flow.Tail && (node.Kind != ScopeNodeKind.Exit || loop is null))
        {
            return;
        }

        if (node.Kind == ScopeNodeKind.Continue)
        {
            Add(depth, "continue;");
        }
        else if (node.Kind == ScopeNodeKind.Exit && loop is null)
        {
            _earlyEnd = true;
            Add(depth, "break;");
        }
        else if (node.Kind == ScopeNodeKind.Exit && flow.ExitSelector is null)
        {
            Add(depth, "break;");
        }
        else if (node.Kind == ScopeNodeKind.Exit)
        {
            var index = loop!.Exits.ToList().IndexOf(node.Node);
            Add(depth, $"{flow.ExitSelector} = {Hlsl.Number(index)}u; break;");
        }
    }

    /// <summary>Arms that meet again: each runs up to the join inside the if/else chain, then the join follows.</summary>
    private int EmitBranches(Flow flow, Choice choice, int depth)
    {
        var arms = Arms(choice, choice.Successors.Where(target => target != choice.Join));
        var complete = arms.Sum(arm => arm.Indices.Count) == choice.Successors.Count;
        var inner = flow with { Stop = choice.Join, Tail = false };
        EmitChain(inner, choice, arms, complete, depth);

        return choice.Join;
    }

    /// <summary>Arms that never meet: the smaller ones inside the chain (they always leave the scope), the largest after it.</summary>
    private int EmitSplit(Flow flow, Choice choice, int depth)
    {
        var arms = Arms(choice, choice.Successors);
        var largest = arms.MaxBy(arm => flow.Scope.ReachWeight(arm.Target))!;
        arms.Remove(largest);
        EmitChain(flow with { Stop = flow.Scope.Sink, Tail = false }, choice, arms, complete: false, depth);

        return largest.Target;
    }

    /// <summary>The distinct targets of a choice with the successor positions that lead to each.</summary>
    private static List<Arm> Arms(Choice choice, IEnumerable<int> targets)
    {
        var wanted = targets.ToHashSet();

        return choice.Successors.Select((target, index) => (Target: target, Index: index))
            .Where(entry => wanted.Contains(entry.Target))
            .GroupBy(entry => entry.Target)
            .Select(group => new Arm(group.Key, group.Select(entry => entry.Index).ToList()))
            .ToList();
    }

    private void EmitChain(Flow flow, Choice choice, List<Arm> arms, bool complete, int depth)
    {
        for (var index = 0; index < arms.Count; index++)
        {
            var condition = Condition(choice, arms[index].Indices);

            if (index == 0)
            {
                Add(depth, $"if ({condition}) {{");
            }
            else if (complete && index == arms.Count - 1)
            {
                Add(depth, "} else {");
            }
            else
            {
                Add(depth, $"}} else if ({condition}) {{");
            }

            EmitFrom(flow, arms[index].Target, depth + 1);
        }

        if (arms.Count > 0)
        {
            Add(depth, "}");
        }
    }

    /// <summary>
    /// The guarded form of a scope: no block is copied. <c>flowN</c> holds the scope-local number of the node to run; the
    /// nodes follow in topological order, so one pass runs a whole path. Exits leave the loop after the last node.
    /// </summary>
    private void EmitGuarded(ScopeGraph scope, string? exitSelector, int depth)
    {
        var guard = "flow" + Hlsl.Number(_guards);
        _guards++;
        Add(depth, $"uint {guard} = {Hlsl.Number(scope.Entry)}u;");

        foreach (var local in scope.CodeOrder())
        {
            Add(depth, $"if ({guard} == {Hlsl.Number(local)}u) {{");
            var loopSelector = EmitNode(scope.Node(local), depth + 1);
            EmitTransfer(scope, guard, local, loopSelector, depth + 1);
            Add(depth, "}");
        }

        if (scope.Loop is null)
        {
            return;
        }

        foreach (var exit in scope.ExitNodes())
        {
            Add(depth, $"if ({guard} == {Hlsl.Number(exit)}u) {{");
            EmitLeave(new Flow(scope, exitSelector, scope.Sink, Tail: false), scope.Node(exit), depth + 1);
            Add(depth, "}");
        }
    }

    /// <summary>Sets the guard of a guarded scope to the node's successor.</summary>
    private void EmitTransfer(ScopeGraph scope, string guard, int local, string? loopSelector, int depth)
    {
        var successors = scope.Successors(local);

        if (successors.Distinct().Count() == 1)
        {
            Add(depth, $"{guard} = {Hlsl.Number(successors[0])}u;");

            return;
        }

        var node = scope.Node(local);

        if (node.Kind == ScopeNodeKind.Block)
        {
            var condition = Condition(new Choice(node, null, successors, scope.Sink), [0]);
            Add(depth, $"{guard} = ({condition}) ? {Hlsl.Number(successors[0])}u : {Hlsl.Number(successors[1])}u;");

            return;
        }

        for (var index = 0; index < successors.Count; index++)
        {
            var test = index == 0 ? "if" : "else if";
            Add(depth, $"{test} ({loopSelector} == {Hlsl.Number(index)}u) {guard} = {Hlsl.Number(successors[index])}u;");
        }
    }

    /// <summary>Emits a block or a nested loop; returns the loop's exit selector, if any.</summary>
    private string? EmitNode(ScopeNode node, int depth)
    {
        if (node.Kind == ScopeNodeKind.Block)
        {
            EmitBlock(node.Node, depth);

            return null;
        }

        return EmitLoop(node.Node, depth);
    }

    /// <summary>Emits a block without its pc assignment.</summary>
    private void EmitBlock(int node, int depth)
    {
        var block = _graph.Blocks[node];

        // The last line sets pc for the dispatch form; a mesh yield keeps it (it tells the phase loop where to resume).
        var count = block.Exit.Kind == BlockExitKind.Yield ? block.Lines.Count : block.Lines.Count - 1;
        var prefix = Indentation(depth);

        for (var index = 0; index < count; index++)
        {
            if (_keep(block.Lines[index]))
            {
                _lines.Add(prefix + block.Lines[index]);
            }
        }

        _emitted += count;

        foreach (var frame in _frames.Where(frame => !frame.Guarded && _emitted - frame.Start > frame.Limit))
        {
            throw new ScopeOverflowException(frame.Id);
        }
    }

    /// <summary>Emits a loop; returns its exit selector when it has several exits.</summary>
    private string? EmitLoop(int header, int depth)
    {
        var loop = _forest.Loop(header);

        if (!_scopes.TryGetValue(header, out var scope))
        {
            scope = new ScopeGraph(_graph, _forest, loop, header);
            _scopes[header] = scope;
        }

        string? selector = null;

        if (loop.Exits.Count > 1)
        {
            selector = "loop_exit" + Hlsl.Number(_loops);
            _loops++;
            Add(depth, $"uint {selector} = 0;");
        }

        var guard = Hlsl.Number(_loopGuards);
        _loopGuards++;

        Add(depth, "[loop] for (;;) {");
        Add(depth + 1, $"if (++loop_budget > {_cap}) {{ loop_cap_hit({guard}, {_cap}); break; }}");
        EmitScope(scope, selector, depth + 1);
        Add(depth, "}");

        return selector;
    }

    /// <summary>Too deep: the innermost structured scope falls back to the guarded form.</summary>
    private void Overflow(string reason)
    {
        var frame = _frames.LastOrDefault(frame => !frame.Guarded) ?? throw new StructuringFailedException(reason);

        throw new ScopeOverflowException(frame.Id);
    }

    /// <summary>The condition under which a choice takes the successors at <paramref name="indices"/>.</summary>
    private string Condition(Choice choice, List<int> indices)
    {
        if (choice.Node.Kind == ScopeNodeKind.Loop)
        {
            return string.Join(" || ", indices.Select(index => $"{choice.LoopSelector} == {Hlsl.Number(index)}u"));
        }

        var condition = _graph.Blocks[choice.Node.Node].Exit.Condition ?? throw new StructuringFailedException("branch without condition");

        // Index 0 is the taken target. Every condition is a single term, optionally negated with a leading '!'.
        if (indices[0] == 0)
        {
            return condition;
        }

        if (condition.StartsWith('!'))
        {
            return condition[1..];
        }

        return "!" + condition;
    }

    private void Add(int depth, string statement)
    {
        _lines.Add(StatementIndent + Indentation(depth) + statement);
    }

    private static string Indentation(int depth)
    {
        return new string(' ', depth * IndentWidth);
    }
}
