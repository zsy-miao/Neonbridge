// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Dxil.Ir;

/// <summary>
/// One instruction. <see cref="Code"/> is the operator, cast, predicate or atomic code, or the index of an
/// <c>extractvalue</c>; <see cref="Targets"/> are a branch's or switch's successors (the switch default first) or a
/// phi's incoming blocks, parallel to its operands.
/// </summary>
public sealed class Instruction : Value
{
    private List<BasicBlock>? _targets;
    private List<Constant>? _cases;

    internal Instruction(Opcode opcode, IrType type, BasicBlock block)
        : base(type)
    {
        Opcode = opcode;
        Block = block;
        Operands = [];
    }

    internal Instruction(Opcode opcode, IrType type, BasicBlock block, int operands)
        : base(type)
    {
        Opcode = opcode;
        Block = block;
        Operands = new List<Value>(operands);
    }

    public Opcode Opcode { get; }

    /// <summary>Written: has an effect or feeds one (set by the writer).</summary>
    internal bool Live { get; set; }

    /// <summary>A removed phi's value (set by SSA construction).</summary>
    internal Value? Replacement { get; set; }

    public BasicBlock Block { get; internal set; }

    public List<Value> Operands { get; }

    public int Code { get; init; }

    public List<BasicBlock> Targets => _targets ??= [];

    /// <summary>A switch's case values, parallel to <see cref="Targets"/> after the default.</summary>
    public List<Constant> Cases => _cases ??= [];

    /// <summary>The called function.</summary>
    public IrFunction? Callee { get; init; }

    /// <summary>The element type a <c>getelementptr</c> indexes or an <c>alloca</c> allocates.</summary>
    public IrType? SourceType { get; init; }

    public bool IsTerminator => Opcode is Opcode.Branch or Opcode.ConditionalBranch or Opcode.Switch or Opcode.Return;

    /// <summary>Whether the instruction must be kept even when nothing reads its result.</summary>
    public bool HasEffect
    {
        get
        {
            if (Opcode is Opcode.Store or Opcode.AtomicRmw or Opcode.CompareExchange || IsTerminator)
            {
                return true;
            }

            return Opcode == Opcode.Call && Callee!.Attribute is not (AttributeSet.ReadNone or AttributeSet.ReadOnly);
        }
    }

    public override string ToString()
    {
        return $"{Opcode} {Code} {Callee?.Name}";
    }
}
