// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Dxil.Ir;

/// <summary>
/// Removes blocks the entry cannot reach, the phi operands that came from them, and instructions without effect whose
/// results nothing reads, so the feature flags and the written module see only the code that runs. Empty blocks that
/// only branch on are folded into their predecessors, which also makes a loop's exiting block its latch (the AMD driver
/// hangs on divergent loops otherwise, as VIctory found).
/// </summary>
public static class DeadCode
{
    public static void Remove(IrFunction function)
    {
        var reachable = Reachable(function.Blocks[0]);
        function.Blocks.RemoveAll(block => !reachable.Contains(block));

        foreach (var phi in function.Blocks.SelectMany(block => block.Phis))
        {
            for (var index = phi.Operands.Count - 1; index >= 0; index--)
            {
                if (!reachable.Contains(phi.Targets[index]))
                {
                    phi.Operands.RemoveAt(index);
                    phi.Targets.RemoveAt(index);
                }
            }
        }

        FoldEmptyBlocks(function);
        MarkLive(function);

        foreach (var block in function.Blocks)
        {
            block.Phis.RemoveAll(phi => !phi.Live);
            block.Body.RemoveAll(instruction => !instruction.Live);

            // The writer marks again from scratch.
            block.Phis.ForEach(phi => phi.Live = false);
            block.Body.ForEach(instruction => instruction.Live = false);
            block.Terminator!.Live = false;
        }
    }

    private static void FoldEmptyBlocks(IrFunction function)
    {
        var predecessors = Predecessors(function);
        var entry = function.Blocks[0];
        var folded = new HashSet<BasicBlock>();

        foreach (var block in function.Blocks)
        {
            var terminator = block.Terminator!;

            if (ReferenceEquals(block, entry) || block.Phis.Count > 0 || block.Body.Count > 0 || terminator.Opcode != Opcode.Branch)
            {
                continue;
            }

            var target = terminator.Targets[0];

            if (!ReferenceEquals(target, block) && TryFold(block, target, predecessors))
            {
                folded.Add(block);
            }
        }

        function.Blocks.RemoveAll(folded.Contains);
    }

    /// <summary>Sends every branch into <paramref name="block"/> straight to <paramref name="target"/>, unless a phi there would get two values from one block.</summary>
    private static bool TryFold(BasicBlock block, BasicBlock target, Dictionary<BasicBlock, HashSet<BasicBlock>> predecessors)
    {
        var sources = predecessors[block];
        var targetSources = predecessors[target];

        if (target.Phis.Count > 0 && sources.Any(targetSources.Contains))
        {
            return false;
        }

        foreach (var phi in target.Phis)
        {
            var index = phi.Targets.IndexOf(block);
            var value = phi.Operands[index];
            phi.Operands.RemoveAt(index);
            phi.Targets.RemoveAt(index);

            // A phi has one entry per edge: a switch may reach the block on several cases.
            foreach (var source in sources)
            {
                var edges = source.Terminator!.Targets.Count(successor => ReferenceEquals(successor, block));

                for (var edge = 0; edge < edges; edge++)
                {
                    phi.Operands.Add(value);
                    phi.Targets.Add(source);
                }
            }
        }

        foreach (var source in sources)
        {
            var targets = source.Terminator!.Targets;

            for (var index = 0; index < targets.Count; index++)
            {
                if (ReferenceEquals(targets[index], block))
                {
                    targets[index] = target;
                }
            }
        }

        targetSources.Remove(block);
        targetSources.UnionWith(sources);

        return true;
    }

    private static Dictionary<BasicBlock, HashSet<BasicBlock>> Predecessors(IrFunction function)
    {
        var predecessors = function.Blocks.ToDictionary(block => block, _ => new HashSet<BasicBlock>());

        foreach (var block in function.Blocks)
        {
            foreach (var successor in block.Successors)
            {
                predecessors[successor].Add(block);
            }
        }

        return predecessors;
    }

    private static HashSet<BasicBlock> Reachable(BasicBlock entry)
    {
        var reachable = new HashSet<BasicBlock> { entry };
        var work = new Stack<BasicBlock>();
        work.Push(entry);

        while (work.Count > 0)
        {
            foreach (var successor in work.Pop().Successors)
            {
                if (reachable.Add(successor))
                {
                    work.Push(successor);
                }
            }
        }

        return reachable;
    }

    /// <summary>Sets <see cref="Instruction.Live"/> on everything with an effect and everything it reads.</summary>
    private static void MarkLive(IrFunction function)
    {
        var work = new Stack<Instruction>();

        foreach (var block in function.Blocks)
        {
            foreach (var instruction in block.Body)
            {
                if (instruction.HasEffect)
                {
                    Mark(instruction, work);
                }
            }

            Mark(block.Terminator!, work);
        }

        while (work.Count > 0)
        {
            foreach (var operand in work.Pop().Operands)
            {
                if (operand is Instruction instruction)
                {
                    Mark(instruction, work);
                }
            }
        }
    }

    private static void Mark(Instruction instruction, Stack<Instruction> work)
    {
        if (!instruction.Live)
        {
            instruction.Live = true;
            work.Push(instruction);
        }
    }
}
