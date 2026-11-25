// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Collections.Frozen;
using FiveStar.Shaders.Dxil.Ops;

namespace FiveStar.Shaders.Dxil.Ir;

/// <summary>
/// The clean-ups after <see cref="ValueNumbering"/> that change the blocks or touch wave operations. A block that
/// only its predecessor's unconditional branch reaches joins that predecessor (every inlined call and structured
/// region ends in such a branch); a branch on a constant or to one block twice becomes an unconditional branch, a phi
/// of one value that value, and what their operands then decide is folded again (<see cref="ConstantFolding"/>).
/// Wave and quad operations that only read are declared nounwind, as dxc declares them, so nothing removed them: one
/// nothing reads goes, and one with the operands of an earlier one in its block, with no effect between them, is that
/// one (a discard or a barrier between them may change the lanes). Two rounds, as a folded branch frees blocks to merge.
/// </summary>
public static class Simplifier
{
    private const int Rounds = 2;

    /// <summary>Wave and quad operations whose value depends only on their operands and the lanes that run them.</summary>
    private static readonly FrozenSet<int> WaveReadOperations = new[]
    {
        DxOpCode.WaveIsFirstLane, DxOpCode.WaveAnyTrue, DxOpCode.WaveAllTrue, DxOpCode.WaveActiveAllEqual,
        DxOpCode.WaveActiveBallot, DxOpCode.WaveReadLaneAt, DxOpCode.WaveReadLaneFirst, DxOpCode.WaveActiveOp,
        DxOpCode.WaveActiveBit, DxOpCode.WavePrefixOp, DxOpCode.WaveAllBitCount, DxOpCode.WavePrefixBitCount,
        DxOpCode.QuadReadLaneAt, DxOpCode.QuadOp,
    }.Select(code => (int)code).ToFrozenSet();

    /// <summary>Simplifies <paramref name="function"/>, numbered and cleaned; cleans it again after each round.</summary>
    public static void Run(IrFunction function, DxilModule module)
    {
        for (var round = 0; round < Rounds && function.Blocks.Count > 0; round++)
        {
            var replaced = new Dictionary<Value, Value>();
            MergeChains(function, replaced);
            var tree = DominatorTree.Of(function);
            var pending = new Stack<BasicBlock>();
            pending.Push(function.Blocks[0]);

            // Dominator-tree preorder: an instruction's operands other than phis are folded before it.
            while (pending.Count > 0)
            {
                var block = pending.Pop();
                Simplify(block, module, replaced);

                foreach (var child in tree.Children(block))
                {
                    pending.Push(child);
                }
            }

            foreach (var block in function.Blocks)
            {
                block.Phis.ForEach(phi => Resolve(phi, replaced));
                block.Body.ForEach(instruction => Resolve(instruction, replaced));
                Resolve(block.Terminator!, replaced);
            }

            RemoveUnreadWaveReads(function);
            DeadCode.Remove(function);
        }
    }

    /// <summary>Appends each block reached only by its predecessor's unconditional branch to that predecessor.</summary>
    private static void MergeChains(IrFunction function, Dictionary<Value, Value> replaced)
    {
        var entry = function.Blocks[0];
        var edges = new Dictionary<BasicBlock, int>();

        foreach (var successor in function.Blocks.SelectMany(block => block.Successors))
        {
            edges[successor] = edges.GetValueOrDefault(successor) + 1;
        }

        var merged = new HashSet<BasicBlock>();

        foreach (var block in function.Blocks)
        {
            if (merged.Contains(block))
            {
                continue;
            }

            while (block.Terminator!.Opcode == Opcode.Branch)
            {
                var next = block.Terminator.Targets[0];

                if (ReferenceEquals(next, block) || ReferenceEquals(next, entry) || edges[next] != 1)
                {
                    break;
                }

                Absorb(block, next, replaced);
                merged.Add(next);
            }
        }

        function.Blocks.RemoveAll(merged.Contains);
    }

    private static void Absorb(BasicBlock block, BasicBlock next, Dictionary<Value, Value> replaced)
    {
        // One incoming edge: every phi has the one value from this block.
        foreach (var phi in next.Phis)
        {
            replaced[phi] = phi.Operands[0];
        }

        foreach (var instruction in next.Body)
        {
            instruction.Block = block;
            block.Body.Add(instruction);
        }

        var terminator = next.Terminator!;
        terminator.Block = block;
        block.Terminator = terminator;

        foreach (var phi in terminator.Targets.Distinct().SelectMany(successor => successor.Phis))
        {
            for (var index = 0; index < phi.Targets.Count; index++)
            {
                if (ReferenceEquals(phi.Targets[index], next))
                {
                    phi.Targets[index] = block;
                }
            }
        }
    }

    /// <summary>Phis of one value, folds, wave reads the block already made, then the block's branch.</summary>
    private static void Simplify(BasicBlock block, DxilModule module, Dictionary<Value, Value> replaced)
    {
        foreach (var phi in block.Phis)
        {
            Resolve(phi, replaced);
            var single = SingleValue(phi);

            if (single is not null)
            {
                replaced[phi] = single;
            }
        }

        var waveReads = new Dictionary<ValueNumbering.Key, Value>();

        block.Body.RemoveAll(instruction => Replace(instruction, module, replaced, waveReads));
        Resolve(block.Terminator!, replaced);
        FoldBranch(block, module);
    }

    /// <summary>True when the instruction is replaced by a constant, an operand or an earlier wave read.</summary>
    private static bool Replace(Instruction instruction, DxilModule module, Dictionary<Value, Value> replaced, Dictionary<ValueNumbering.Key, Value> waveReads)
    {
        Resolve(instruction, replaced);
        var folded = Fold(instruction, module);

        if (folded is not null)
        {
            replaced[instruction] = folded;

            return true;
        }

        if (!IsWaveRead(instruction))
        {
            if (instruction.HasEffect)
            {
                waveReads.Clear();
            }

            return false;
        }

        var key = new ValueNumbering.Key(instruction);

        if (waveReads.TryGetValue(key, out var earlier))
        {
            replaced[instruction] = earlier;

            return true;
        }

        waveReads[key] = instruction;

        return false;
    }

    /// <summary>The value <see cref="ConstantFolding"/> gives the instruction now that its operands are resolved, or null.</summary>
    private static Value? Fold(Instruction instruction, DxilModule module)
    {
        var operands = instruction.Operands;

        switch (instruction.Opcode)
        {
            case Opcode.Binary:
                return ConstantFolding.Binary(module, (BinaryOp)instruction.Code, operands[0], operands[1]);
            case Opcode.Compare:
                return ConstantFolding.Compare(module, (Predicate)instruction.Code, operands[0], operands[1]);
            case Opcode.Cast:
                return ConstantFolding.Cast(module, (CastOp)instruction.Code, operands[0], instruction.Type);
            case Opcode.Select:
                return ConstantFolding.Select(operands[0], operands[1], operands[^1]);
            default:
                return null;
        }
    }

    private static bool IsWaveRead(Instruction instruction)
    {
        var operands = instruction.Operands;

        if (instruction.Opcode != Opcode.Call || operands.Count == 0 || operands[0] is not Constant code || !code.IsInteger)
        {
            return false;
        }

        return WaveReadOperations.Contains((int)code.Bits);
    }

    /// <summary>Removes the wave reads nothing with an effect depends on; <see cref="DeadCode"/> then removes their operands.</summary>
    private static void RemoveUnreadWaveReads(IrFunction function)
    {
        var work = new Stack<Instruction>();

        foreach (var block in function.Blocks)
        {
            foreach (var instruction in block.Body.Where(instruction => instruction.HasEffect && !IsWaveRead(instruction)))
            {
                MarkLive(instruction, work);
            }

            MarkLive(block.Terminator!, work);
        }

        while (work.Count > 0)
        {
            foreach (var operand in work.Pop().Operands)
            {
                if (operand is Instruction instruction)
                {
                    MarkLive(instruction, work);
                }
            }
        }

        foreach (var block in function.Blocks)
        {
            block.Body.RemoveAll(instruction => IsWaveRead(instruction) && !instruction.Live);
            block.Phis.ForEach(phi => phi.Live = false);
            block.Body.ForEach(instruction => instruction.Live = false);
            block.Terminator!.Live = false;
        }
    }

    private static void MarkLive(Instruction instruction, Stack<Instruction> work)
    {
        if (!instruction.Live)
        {
            instruction.Live = true;
            work.Push(instruction);
        }
    }

    /// <summary>The one value every incoming edge of <paramref name="phi"/> brings (the phi itself aside), or null.</summary>
    private static Value? SingleValue(Instruction phi)
    {
        Value? single = null;

        foreach (var operand in phi.Operands)
        {
            if (ReferenceEquals(operand, phi) || ReferenceEquals(operand, single))
            {
                continue;
            }

            if (single is not null)
            {
                return null;
            }

            single = operand;
        }

        return single;
    }

    /// <summary>A conditional branch on a constant, or to one block twice, becomes a branch; the dropped edge leaves the phis.</summary>
    private static void FoldBranch(BasicBlock block, DxilModule module)
    {
        var terminator = block.Terminator!;

        if (terminator.Opcode != Opcode.ConditionalBranch)
        {
            return;
        }

        var whenTrue = terminator.Targets[0];
        var whenFalse = terminator.Targets[1];
        var constant = terminator.Operands[0] as Constant;
        var known = constant is not null && constant.IsInteger;

        if (!known && !ReferenceEquals(whenTrue, whenFalse))
        {
            return;
        }

        var kept = whenTrue;
        var dropped = whenFalse;

        if (known && constant!.Bits == 0)
        {
            kept = whenFalse;
            dropped = whenTrue;
        }

        foreach (var phi in dropped.Phis)
        {
            var index = phi.Targets.IndexOf(block);
            phi.Operands.RemoveAt(index);
            phi.Targets.RemoveAt(index);
        }

        var branch = new Instruction(Opcode.Branch, module.Types.Void, block);
        branch.Targets.Add(kept);
        block.Terminator = branch;
    }

    private static void Resolve(Instruction instruction, Dictionary<Value, Value> replaced)
    {
        var operands = instruction.Operands;

        for (var index = 0; index < operands.Count; index++)
        {
            var value = operands[index];

            while (replaced.TryGetValue(value, out var next))
            {
                value = next;
            }

            operands[index] = value;
        }
    }
}
