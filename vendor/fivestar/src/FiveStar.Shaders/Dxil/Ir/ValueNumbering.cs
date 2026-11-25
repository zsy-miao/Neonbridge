// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Dxil.Ir;

/// <summary>
/// Common-subexpression removal over the dominator tree: an instruction that computes what an instruction in a
/// dominating block already computed is replaced by that one. Pure instructions (arithmetic, comparisons, casts,
/// selects, extracts, addresses, readnone <c>dx.op</c> calls and constant-buffer loads) match in any dominated block;
/// other memory reads (raw buffer and texture loads, samples) only later in the same block with no write, atomic or
/// call with an effect between them. Wave operations are not readnone (their result depends on the active lanes) and
/// never match. <see cref="DeadCode"/> then drops what is
/// no longer read. The translator repeats the same handle creations, address-map lookups and range arithmetic for
/// every access to one buffer; a large compute program loses about a fifth of its instructions.
/// </summary>
public static class ValueNumbering
{
    private const string CbufferLoad = "dx.op.cbufferLoadLegacy";

    public static void Run(IrFunction function)
    {
        if (function.Blocks.Count > 0)
        {
            new Pass(function).Run();
        }
    }

    private static Kind KindOf(Instruction instruction)
    {
        switch (instruction.Opcode)
        {
            case Opcode.Binary or Opcode.Cast or Opcode.Compare or Opcode.Select or Opcode.ExtractValue or Opcode.GetElementPtr:
                return Kind.Pure;
            case Opcode.Call:
                return CallKind(instruction.Callee!);
            case Opcode.Load or Opcode.Alloca or Opcode.Phi:
                return Kind.Other;
            default:
                return Kind.Effect;
        }
    }

    private static Kind CallKind(IrFunction callee)
    {
        if (callee.Attribute == AttributeSet.ReadNone || callee.Name.StartsWith(CbufferLoad, StringComparison.Ordinal))
        {
            return Kind.Pure;
        }

        if (callee.Attribute == AttributeSet.ReadOnly)
        {
            return Kind.Read;
        }

        return Kind.Effect;
    }

    private static void Rewrite(Instruction instruction, Dictionary<Value, Value> replaced)
    {
        if (replaced.Count == 0)
        {
            return;
        }

        var operands = instruction.Operands;

        for (var index = 0; index < operands.Count; index++)
        {
            var value = operands[index];

            while (replaced.TryGetValue(value, out var replacement))
            {
                value = replacement;
            }

            operands[index] = value;
        }
    }

    /// <summary>
    /// Phis read values of later blocks over back edges: rewritten once every block is numbered. A phi whose
    /// operands are then all one value (itself aside) is that value, which can make others trivial in turn.
    /// </summary>
    private static void RewritePhis(IrFunction function, Dictionary<Value, Value> replaced)
    {
        bool changed = true;

        while (changed)
        {
            changed = false;

            foreach (var block in function.Blocks)
            {
                foreach (var phi in block.Phis)
                {
                    Rewrite(phi, replaced);
                }

                changed |= block.Phis.RemoveAll(phi => Trivial(phi, replaced)) > 0;
            }
        }

        // Instructions read the removed phis too: one more pass over everything.
        foreach (var block in function.Blocks)
        {
            block.Phis.ForEach(phi => Rewrite(phi, replaced));
            block.Body.ForEach(instruction => Rewrite(instruction, replaced));
            Rewrite(block.Terminator!, replaced);
        }
    }

    private static bool Trivial(Instruction phi, Dictionary<Value, Value> replaced)
    {
        Value? only = null;

        foreach (var operand in phi.Operands)
        {
            if (ReferenceEquals(operand, phi) || ReferenceEquals(operand, only))
            {
                continue;
            }

            if (only is not null)
            {
                return false;
            }

            only = operand;
        }

        if (only is null)
        {
            return false;
        }

        replaced[phi] = only;

        return true;
    }

    /// <summary>One run: the walk over the dominator tree with its tables.</summary>
    private sealed class Pass
    {
        private readonly IrFunction _function;
        private readonly BasicBlock _entry;
        private readonly Dictionary<Value, Value> _replaced = [];
        private readonly Dictionary<Key, Value> _available = [];
        private readonly HashSet<Value> _entryValues = [];
        private readonly List<Instruction> _hoisted = [];

        public Pass(IrFunction function)
        {
            _function = function;
            _entry = function.Blocks[0];
        }

        public void Run()
        {
            var tree = DominatorTree.Of(_function);
            var scopes = new Stack<(BasicBlock Block, List<Key> Added, bool Entered)>();
            scopes.Push((_entry, [], false));

            while (scopes.Count > 0)
            {
                var (block, added, entered) = scopes.Pop();

                if (entered)
                {
                    added.ForEach(key => _available.Remove(key));

                    continue;
                }

                scopes.Push((block, added, true));
                Number(block, added);

                foreach (var child in tree.Children(block))
                {
                    scopes.Push((child, [], false));
                }
            }

            _entry.Body.AddRange(_hoisted);
            RewritePhis(_function, _replaced);
        }

        private void Number(BasicBlock block, List<Key> added)
        {
            var local = new Dictionary<Key, Value>();

            foreach (var phi in block.Phis)
            {
                Rewrite(phi, _replaced);
            }

            block.Body.RemoveAll(instruction => Replace(block, instruction, added, local));
            Rewrite(block.Terminator!, _replaced);

            if (ReferenceEquals(block, _entry))
            {
                _entryValues.UnionWith(block.Body);
            }
        }

        /// <summary>
        /// Rewrites the operands, then replaces the instruction by an earlier equal one or, when it is pure and reads
        /// only constants and entry values, moves it to the entry, where every block sees it (true: remove it here).
        /// </summary>
        private bool Replace(BasicBlock block, Instruction instruction, List<Key> added, Dictionary<Key, Value> local)
        {
            Rewrite(instruction, _replaced);
            var kind = KindOf(instruction);

            if (kind == Kind.Effect)
            {
                local.Clear();

                return false;
            }

            if (kind == Kind.Other)
            {
                return false;
            }

            var key = new Key(instruction);
            var table = kind == Kind.Pure ? _available : local;

            if (table.TryGetValue(key, out var earlier))
            {
                _replaced[instruction] = earlier;

                return true;
            }

            table[key] = instruction;

            if (kind == Kind.Pure && !ReferenceEquals(block, _entry) && Hoistable(instruction))
            {
                // Its key stays available to the end: no block scope removes it.
                _hoisted.Add(instruction);
                _entryValues.Add(instruction);

                return true;
            }

            if (kind == Kind.Pure)
            {
                added.Add(key);
            }

            return false;
        }

        /// <summary>Pure, cannot trap (no division), and reads only constants and values the entry computes.</summary>
        private bool Hoistable(Instruction instruction)
        {
            if (instruction.Opcode == Opcode.Binary && (BinaryOp)instruction.Code is BinaryOp.UDiv or BinaryOp.SDiv or BinaryOp.URem or BinaryOp.SRem)
            {
                return false;
            }

            foreach (var operand in instruction.Operands)
            {
                if (operand is not (Constant or IrFunction) && !_entryValues.Contains(operand))
                {
                    return false;
                }
            }

            return true;
        }
    }

    private enum Kind
    {
        /// <summary>Its value depends only on its operands: matched in dominated blocks.</summary>
        Pure,

        /// <summary>A memory read: matched later in its block until a write.</summary>
        Read,

        /// <summary>Writes or synchronizes: never matched, ends the block's reads.</summary>
        Effect,

        /// <summary>Not matched (loads from local or shared memory, phis, allocas), no effect.</summary>
        Other,
    }

    /// <summary>What an instruction computes: its operator, codes, types and operands (by reference; constants are interned).</summary>
    internal sealed class Key : IEquatable<Key>
    {
        private readonly Instruction _instruction;
        private readonly int _hash;

        public Key(Instruction instruction)
        {
            _instruction = instruction;
            var hash = HashCode.Combine(instruction.Opcode, instruction.Code, instruction.Type, instruction.Callee, instruction.SourceType);

            foreach (var operand in instruction.Operands)
            {
                hash = HashCode.Combine(hash, operand);
            }

            _hash = hash;
        }

        public bool Equals(Key? other)
        {
            if (other is null)
            {
                return false;
            }

            var a = _instruction;
            var b = other._instruction;

            if (a.Opcode != b.Opcode || a.Code != b.Code || !ReferenceEquals(a.Type, b.Type) || !ReferenceEquals(a.Callee, b.Callee)
                || !ReferenceEquals(a.SourceType, b.SourceType) || a.Operands.Count != b.Operands.Count)
            {
                return false;
            }

            for (var index = 0; index < a.Operands.Count; index++)
            {
                if (!ReferenceEquals(a.Operands[index], b.Operands[index]))
                {
                    return false;
                }
            }

            return true;
        }

        public override bool Equals(object? obj)
        {
            return Equals(obj as Key);
        }

        public override int GetHashCode()
        {
            return _hash;
        }
    }
}
