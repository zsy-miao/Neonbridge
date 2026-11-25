// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Translation.Structure;

/// <summary>
/// One level of a structured program: the body of a loop, or the top level of a region. Loops nested directly inside are
/// collapsed to single nodes, branches back to the loop header become a continue node and branches out of the scope
/// become exit nodes, so the graph is acyclic. Every continue and exit leads to one sink.
/// </summary>
internal sealed class ScopeGraph
{
    // A shared exit tail up to this many lines is copied into each arm rather than used as a join.
    private const int ExitTailLines = 64;

    private readonly FlowGraph _graph;
    private readonly LoopForest _forest;
    private readonly List<ScopeNode> _nodes = [];
    private readonly List<List<int>> _successors = [];
    private readonly Dictionary<ScopeNode, int> _locals = [];
    private readonly DominatorTree _postDominators;
    private readonly DominatorTree _mainPostDominators;

    /// <summary>The scope of <paramref name="loop"/>, or the top level from <paramref name="entry"/> when it is null.</summary>
    public ScopeGraph(FlowGraph graph, LoopForest forest, NaturalLoop? loop, int entry)
    {
        _graph = graph;
        _forest = forest;
        Loop = loop;

        var start = loop is null ? Representative(entry) : new ScopeNode(ScopeNodeKind.Block, entry);
        Entry = Add(start);
        Build();
        Sink = Add(new ScopeNode(ScopeNodeKind.Sink, 0));

        for (var local = 0; local < Sink; local++)
        {
            if (_nodes[local].Kind is ScopeNodeKind.Continue or ScopeNodeKind.Exit)
            {
                _successors[local].Add(Sink);
            }
        }

        _postDominators = new DominatorTree(Reverse(local => _successors[local]), Sink);

        if (_postDominators.Order.Count != _nodes.Count)
        {
            throw new StructuringFailedException("loop without exit");
        }

        _mainPostDominators = new DominatorTree(Reverse(MainSuccessors()), Sink);
    }

    /// <summary>The loop whose body this is; null at the top level.</summary>
    public NaturalLoop? Loop { get; }

    public int Entry { get; }

    public int Sink { get; }

    public ScopeNode Node(int local)
    {
        return _nodes[local];
    }

    /// <summary>A block's targets (taken first) or a loop's exits, in the order of <see cref="NaturalLoop.Exits"/>.</summary>
    public IReadOnlyList<int> Successors(int local)
    {
        return _successors[local];
    }

    /// <summary>
    /// Where the arms of a branching node continue together: its immediate post-dominator when that is a block or loop.
    /// Otherwise some arm leaves the scope (a break, an early end); the join is then the post-dominator of the main
    /// graph: inside a loop the edges out of the loop are set aside (every body node still reaches the back edge), at the
    /// top level small exit tails (code that only leads to the end, such as a kill-and-end block) are, and those are
    /// copied into the arms that reach them. Without one, the post-dominating continue or exit node, or null when the
    /// arms leave in different ways.
    /// </summary>
    public int? Join(int local)
    {
        var postDominator = _postDominators.Immediate(local) ?? Sink;

        if (IsCode(postDominator))
        {
            return postDominator;
        }

        var main = _mainPostDominators.Immediate(local);

        if (main is not null && IsCode(main.Value))
        {
            return main;
        }

        if (postDominator == Sink)
        {
            return null;
        }

        return postDominator;
    }

    /// <summary>The blocks and loops of the scope in topological order (every edge goes forward).</summary>
    public IEnumerable<int> CodeOrder()
    {
        return new DominatorTree(_successors, Entry).Order.Where(IsCode);
    }

    /// <summary>The exit nodes of the scope.</summary>
    public IEnumerable<int> ExitNodes()
    {
        return Enumerable.Range(0, _nodes.Count).Where(local => _nodes[local].Kind == ScopeNodeKind.Exit);
    }

    /// <summary>The lines of every block of the scope, nested loops included.</summary>
    public int TotalWeight()
    {
        return Enumerable.Range(0, _nodes.Count).Where(IsCode).Sum(NodeWeight);
    }

    /// <summary>The lines of the blocks reachable from <paramref name="local"/> within the scope.</summary>
    public int ReachWeight(int local)
    {
        return Reach(local).Sum(NodeWeight);
    }

    /// <summary>Whether everything reachable from <paramref name="local"/> is small and leads only out of the scope.</summary>
    private bool IsExitTail(int local)
    {
        var reached = new HashSet<int>();
        var pending = new Stack<int>();
        var weight = 0;
        pending.Push(local);

        while (pending.Count > 0)
        {
            var node = pending.Pop();
            var kind = _nodes[node].Kind;

            if (kind == ScopeNodeKind.Continue)
            {
                return false;
            }

            if (kind is not ScopeNodeKind.Block and not ScopeNodeKind.Loop || !reached.Add(node))
            {
                continue;
            }

            weight += NodeWeight(node);

            if (weight > ExitTailLines)
            {
                return false;
            }

            foreach (var successor in _successors[node])
            {
                pending.Push(successor);
            }
        }

        return true;
    }

    /// <summary>The lines of one block, or of every block of a collapsed loop.</summary>
    private int NodeWeight(int local)
    {
        var node = _nodes[local];

        if (node.Kind == ScopeNodeKind.Loop)
        {
            return _forest.Loop(node.Node).Body.Sum(block => _graph.Blocks[block].Lines.Count);
        }

        return _graph.Blocks[node.Node].Lines.Count;
    }

    /// <summary>Whether a path leads from <paramref name="local"/> to <paramref name="target"/>.</summary>
    public bool Reaches(int local, int target)
    {
        var reached = new HashSet<int>();
        var pending = new Stack<int>();
        pending.Push(local);

        while (pending.Count > 0)
        {
            var node = pending.Pop();

            if (node == target)
            {
                return true;
            }

            if (!reached.Add(node))
            {
                continue;
            }

            foreach (var successor in _successors[node])
            {
                pending.Push(successor);
            }
        }

        return false;
    }

    /// <summary>The blocks and loops reachable from <paramref name="local"/> (itself included).</summary>
    private HashSet<int> Reach(int local)
    {
        var reached = new HashSet<int>();
        var pending = new Stack<int>();
        pending.Push(local);

        while (pending.Count > 0)
        {
            var node = pending.Pop();

            if (_nodes[node].Kind is not ScopeNodeKind.Block and not ScopeNodeKind.Loop || !reached.Add(node))
            {
                continue;
            }

            foreach (var successor in _successors[node])
            {
                pending.Push(successor);
            }
        }

        return reached;
    }

    private void Build()
    {
        var pending = new Queue<int>();
        pending.Enqueue(Entry);

        while (pending.Count > 0)
        {
            var local = pending.Dequeue();
            var node = _nodes[local];
            var targets = node.Kind == ScopeNodeKind.Block ? _graph.Successors[node.Node] : _forest.Loop(node.Node).Exits;

            foreach (var target in targets)
            {
                var key = Map(target);

                if (!_locals.TryGetValue(key, out var successor))
                {
                    successor = Add(key);

                    if (key.Kind is ScopeNodeKind.Block or ScopeNodeKind.Loop)
                    {
                        pending.Enqueue(successor);
                    }
                }

                _successors[local].Add(successor);
            }
        }
    }

    private ScopeNode Map(int target)
    {
        if (Loop is not null && target == Loop.Header)
        {
            return new ScopeNode(ScopeNodeKind.Continue, target);
        }

        var inside = Loop is null ? target != _graph.End : Loop.Body.Contains(target);

        if (!inside)
        {
            return new ScopeNode(ScopeNodeKind.Exit, target);
        }

        return Representative(target);
    }

    /// <summary>The block itself, or the loop nested directly in this scope that holds it.</summary>
    private ScopeNode Representative(int node)
    {
        var scope = Loop?.Header;
        var header = _forest.Innermost(node);

        if (header == scope || header is null)
        {
            return new ScopeNode(ScopeNodeKind.Block, node);
        }

        var current = header.Value;

        while (_forest.Loop(current).Parent != scope)
        {
            current = _forest.Loop(current).Parent ?? throw new StructuringFailedException("loop outside its scope");
        }

        return new ScopeNode(ScopeNodeKind.Loop, current);
    }

    private int Add(ScopeNode node)
    {
        var local = _nodes.Count;
        _nodes.Add(node);
        _successors.Add([]);
        _locals[node] = local;

        return local;
    }

    private bool IsCode(int local)
    {
        return _nodes[local].Kind is ScopeNodeKind.Block or ScopeNodeKind.Loop;
    }

    /// <summary>
    /// The successors without the edges out of the loop (in a loop scope) and without the exit tails (at the top level):
    /// a node left without successors leads to the sink, and the tails themselves have none (they are left out of the
    /// main post-dominator tree).
    /// </summary>
    private Func<int, IEnumerable<int>> MainSuccessors()
    {
        var tails = Enumerable.Range(0, _nodes.Count).Where(local => IsCode(local) && IsExitTail(local)).ToHashSet();
        var main = new List<int>[_nodes.Count];

        for (var local = 0; local < _nodes.Count; local++)
        {
            main[local] = tails.Contains(local) ? [] : _successors[local].Where(successor => IsMain(successor, tails)).ToList();

            if (main[local].Count == 0 && !tails.Contains(local) && local != Sink)
            {
                main[local].Add(Sink);
            }
        }

        return local => main[local];
    }

    private bool IsMain(int local, HashSet<int> tails)
    {
        if (Loop is not null && _nodes[local].Kind == ScopeNodeKind.Exit)
        {
            return false;
        }

        return !tails.Contains(local);
    }

    private List<int>[] Reverse(Func<int, IEnumerable<int>> successors)
    {
        var predecessors = new List<int>[_nodes.Count];

        for (var local = 0; local < _nodes.Count; local++)
        {
            predecessors[local] = [];
        }

        for (var local = 0; local < _nodes.Count; local++)
        {
            foreach (var successor in successors(local))
            {
                predecessors[successor].Add(local);
            }
        }

        return predecessors;
    }
}
