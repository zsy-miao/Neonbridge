// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Dxil.Ir;

/// <summary>A basic block: phis, then the body, then one terminator.</summary>
public sealed class BasicBlock
{
    internal BasicBlock(int number)
    {
        Number = number;
    }

    /// <summary>The creation number (not the written position).</summary>
    public int Number { get; }

    public List<Instruction> Phis { get; } = [];

    public List<Instruction> Body { get; } = [];

    public Instruction? Terminator { get; set; }

    /// <summary>The written position (-1: unreachable, left out).</summary>
    internal int Position { get; set; } = -1;

    public bool IsTerminated => Terminator is not null;

    public IEnumerable<BasicBlock> Successors
    {
        get
        {
            if (Terminator is null)
            {
                return [];
            }

            return Terminator.Targets;
        }
    }

    public override string ToString()
    {
        return $"bb{Number}";
    }
}
