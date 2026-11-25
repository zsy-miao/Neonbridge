// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Dxil.Ir;

namespace FiveStar.Shaders.Dxil.Lowering;

/// <summary>
/// Blocks and branches with their SSA edges, the targets of <c>break</c> and <c>continue</c>, and the exits of inlined
/// functions. Code after a jump goes to a fresh unreachable block, which the writer leaves out.
/// </summary>
public sealed class ControlFlow
{
    private readonly IrBuilder _builder;
    private readonly SsaBuilder _ssa;
    private Stack<(BasicBlock Break, BasicBlock? Continue)> _loops = new();
    private readonly Stack<FunctionFrame> _frames = new();

    public ControlFlow(IrBuilder builder, SsaBuilder ssa)
    {
        _builder = builder;
        _ssa = ssa;
    }

    public BasicBlock Current => _builder.Block;

    public FunctionFrame Frame => _frames.Peek();

    public BasicBlock NewBlock()
    {
        return _ssa.NewBlock();
    }

    /// <summary>Continues in <paramref name="block"/>.</summary>
    public void Enter(BasicBlock block)
    {
        _builder.Block = block;
    }

    /// <summary>The first block of the function; every other block without predecessors is unreachable.</summary>
    public BasicBlock? Entry { get; set; }

    /// <summary>Whether the current block can never run (code after a jump): its branches add no SSA edges.</summary>
    public bool IsDead => !ReferenceEquals(Current, Entry) && _ssa.PredecessorCount(Current) == 0;

    /// <summary>Branches from the current block (unless it already ended).</summary>
    public void Jump(BasicBlock target)
    {
        if (Current.IsTerminated)
        {
            return;
        }

        Edge(target);
        _builder.Branch(target);
    }

    public void Branch(Value condition, BasicBlock whenTrue, BasicBlock whenFalse)
    {
        Edge(whenTrue);
        Edge(whenFalse);
        _builder.ConditionalBranch(condition, whenTrue, whenFalse);
    }

    public void Switch(Value condition, BasicBlock fallback, IReadOnlyList<(Constant Value, BasicBlock Target)> cases)
    {
        Edge(fallback);

        foreach (var (_, target) in cases)
        {
            Edge(target);
        }

        _builder.Switch(condition, fallback, cases);
    }

    private void Edge(BasicBlock target)
    {
        if (!IsDead)
        {
            _ssa.AddEdge(Current, target);
        }
    }

    /// <summary>After a jump: later code is unreachable until the next block is entered.</summary>
    public void Unreachable()
    {
        var block = NewBlock();
        _ssa.Seal(block);
        Enter(block);
    }

    public void Seal(BasicBlock block)
    {
        _ssa.Seal(block);
    }

    public void PushLoop(BasicBlock exit, BasicBlock? next)
    {
        _loops.Push((exit, next));
    }

    public void PopLoop()
    {
        _loops.Pop();
    }

    public void Break()
    {
        if (_loops.Count == 0)
        {
            throw new DxilBackendException("break outside a loop or switch");
        }

        Jump(_loops.Peek().Break);
        Unreachable();
    }

    public void Continue()
    {
        var target = _loops.FirstOrDefault(loop => loop.Continue is not null).Continue
            ?? throw new DxilBackendException("continue outside a loop");
        Jump(target);
        Unreachable();
    }

    public void PushFrame(FunctionFrame frame)
    {
        _frames.Push(frame);
    }

    public void PopFrame()
    {
        _frames.Pop();
    }

    /// <summary>Loops and switches of the caller are not targets inside an inlined function.</summary>
    public Stack<(BasicBlock Break, BasicBlock? Continue)> SaveLoops()
    {
        var saved = _loops;
        _loops = new Stack<(BasicBlock Break, BasicBlock? Continue)>();

        return saved;
    }

    public void RestoreLoops(Stack<(BasicBlock Break, BasicBlock? Continue)> saved)
    {
        _loops = saved;
    }
}
