// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Dxil.Bitcode;

namespace FiveStar.Shaders.Dxil.Ir;

/// <summary>
/// Writes the entry function's block: blocks in reverse postorder from the entry, live instructions only, operands as
/// relative value numbers (a phi's signed, since it may read a later value).
/// </summary>
internal sealed class FunctionBodyWriter
{
    private const int Width = 4;
    private const int ThirdOperand = 2;
    private const int VbrWidth = 6;
    private const int OperatorWidth = 4;

    // A call record's attribute list, flags, function type and callee before the arguments.
    private const int CallFields = 4;

    private readonly BitstreamWriter _out;
    private readonly IrFunction _function;
    private readonly TypeTable _types;
    private readonly List<Instruction> _live = [];
    private readonly Dictionary<FunctionCode, int> _abbreviations = [];
    private List<BasicBlock> _layout = [];
    private int _base;
    private int _next;

    public FunctionBodyWriter(BitstreamWriter output, IrFunction function, TypeTable types)
    {
        _out = output;
        _function = function;
        _types = types;
    }

    /// <summary>Lays the blocks out and finds the live instructions.</summary>
    public void Analyze()
    {
        _layout = ReversePostorder(_function.Blocks[0]);

        for (var index = 0; index < _layout.Count; index++)
        {
            _layout[index].Position = index;
        }

        MarkLive();
    }

    /// <summary>The functions the live instructions call and the globals they address.</summary>
    public HashSet<Value> Referenced()
    {
        var referenced = new HashSet<Value>();

        foreach (var instruction in _live)
        {
            if (instruction.Callee is not null)
            {
                referenced.Add(instruction.Callee);
            }

            foreach (var operand in instruction.Operands)
            {
                if (operand is GlobalVariable global)
                {
                    referenced.Add(global);
                }
            }
        }

        return referenced;
    }

    /// <summary>Numbers the live results after the <paramref name="moduleValues"/> module-level values.</summary>
    public void NumberInstructions(int moduleValues)
    {
        _base = moduleValues;
        var next = _base;

        foreach (var block in _layout)
        {
            next = Number(block.Phis, next);
            next = Number(block.Body, next);
        }
    }

    private static int Number(List<Instruction> instructions, int next)
    {
        foreach (var instruction in instructions)
        {
            if (instruction.Live && !instruction.Type.IsVoid)
            {
                instruction.Id = next;
                next++;
            }
        }

        return next;
    }

    public void Write()
    {
        _out.Enter(BlockId.Function, Width);
        DefineAbbreviations();
        Record(FunctionCode.DeclareBlocks, [(ulong)_layout.Count]);
        _next = _base;

        foreach (var block in _layout)
        {
            WriteLive(block.Phis);
            WriteLive(block.Body);
            WriteInstruction(block.Terminator!);
        }

        _out.Exit();
    }

    /// <summary>
    /// The abbreviations of the frequent records: every field a VBR of 6 bits, an operator or alignment in 4, the
    /// call's attribute list and explicit-type flag as literals, and the variable operand lists as arrays.
    /// </summary>
    private void DefineAbbreviations()
    {
        var vbr = AbbreviationOperand.Vbr(VbrWidth);
        var small = AbbreviationOperand.Fixed(OperatorWidth);
        var flag = AbbreviationOperand.Fixed(1);
        var array = AbbreviationOperand.Array();
        Define(FunctionCode.Call, AbbreviationOperand.Literal(0), AbbreviationOperand.Literal(ModuleWriter.Records.ExplicitType), vbr, array, vbr);
        Define(FunctionCode.Binary, vbr, vbr, small);

        Define(FunctionCode.Cast, vbr, vbr, small);
        Define(FunctionCode.Compare, vbr, vbr, vbr);
        Define(FunctionCode.Select, vbr, vbr, vbr);
        Define(FunctionCode.ExtractValue, vbr, vbr);
        Define(FunctionCode.Phi, array, vbr);
        Define(FunctionCode.Load, vbr, vbr, small, flag);

        Define(FunctionCode.Store, vbr, vbr, small, flag);
        Define(FunctionCode.Branch, array, vbr);
        Define(FunctionCode.GetElementPtr, array, vbr);
    }

    private void Define(FunctionCode code, params AbbreviationOperand[] fields)
    {
        _abbreviations[code] = _out.DefineAbbreviation([AbbreviationOperand.Literal((ulong)code), .. fields]);
    }

    /// <summary>A record in its abbreviation when it has one.</summary>
    private void Record(FunctionCode code, ReadOnlySpan<ulong> operands)
    {
        if (_abbreviations.TryGetValue(code, out var abbreviation))
        {
            _out.Abbreviated(abbreviation, operands);

            return;
        }

        _out.Record((int)code, operands);
    }

    /// <summary>Writes the live instructions of a list in order, numbering their results.</summary>
    private void WriteLive(List<Instruction> instructions)
    {
        foreach (var instruction in instructions)
        {
            if (!instruction.Live)
            {
                continue;
            }

            WriteInstruction(instruction);

            if (!instruction.Type.IsVoid)
            {
                _next++;
            }
        }
    }

    private static List<BasicBlock> ReversePostorder(BasicBlock entry)
    {
        var seen = new HashSet<BasicBlock> { entry };
        var post = new List<BasicBlock>();
        var stack = new Stack<(BasicBlock Block, int Next)>();
        stack.Push((entry, 0));

        while (stack.Count > 0)
        {
            var (block, next) = stack.Pop();
            var successors = block.Terminator!.Targets;

            if (next < successors.Count)
            {
                stack.Push((block, next + 1));

                if (seen.Add(successors[next]))
                {
                    stack.Push((successors[next], 0));
                }

                continue;
            }

            post.Add(block);
        }

        post.Reverse();

        return post;
    }

    private void MarkLive()
    {
        var work = new Stack<Instruction>();

        foreach (var block in _layout)
        {
            foreach (var instruction in block.Body)
            {
                if (instruction.HasEffect)
                {
                    Mark(instruction, work);
                }
            }

            foreach (var operand in block.Terminator!.Operands)
            {
                Mark(operand, work);
            }
        }

        while (work.Count > 0)
        {
            foreach (var operand in work.Pop().Operands)
            {
                Mark(operand, work);
            }
        }
    }

    private void Mark(Value value, Stack<Instruction> work)
    {
        if (value is Instruction instruction && !instruction.Live)
        {
            instruction.Live = true;
            _live.Add(instruction);
            work.Push(instruction);
        }
    }

    private ulong Relative(Value value)
    {
        var id = value.Id;

        if (id < 0)
        {
            throw new InvalidOperationException($"an operand that is not written: {value}");
        }

        if (id >= _next)
        {
            throw new InvalidOperationException($"forward reference outside a phi: {value}");
        }

        return (ulong)(_next - id);
    }

    private void WriteInstruction(Instruction instruction)
    {
        var operands = instruction.Operands;

        switch (instruction.Opcode)
        {
            case Opcode.Binary:
                Record(FunctionCode.Binary, [Relative(operands[0]), Relative(operands[1]), (ulong)instruction.Code]);
                break;
            case Opcode.Cast:
                Record(FunctionCode.Cast, [Relative(operands[0]), (ulong)instruction.Type.Id, (ulong)instruction.Code]);
                break;
            case Opcode.Compare:
                Record(FunctionCode.Compare, [Relative(operands[0]), Relative(operands[1]), (ulong)instruction.Code]);
                break;
            case Opcode.Select:
                Record(FunctionCode.Select, [Relative(operands[1]), Relative(operands[ThirdOperand]), Relative(operands[0])]);
                break;
            case Opcode.ExtractValue:
                Record(FunctionCode.ExtractValue, [Relative(operands[0]), (ulong)instruction.Code]);
                break;
            default:
                WriteMemoryOrControl(instruction);
                break;
        }
    }

    private void WriteMemoryOrControl(Instruction instruction)
    {
        var operands = instruction.Operands;

        switch (instruction.Opcode)
        {
            case Opcode.Load:
                Record(FunctionCode.Load, [Relative(operands[0]), (ulong)instruction.Type.Id, ModuleWriter.Records.Alignment, 0]);
                break;
            case Opcode.Store:
                Record(FunctionCode.Store, [Relative(operands[0]), Relative(operands[1]), ModuleWriter.Records.Alignment, 0]);
                break;
            case Opcode.AtomicRmw:
                Record(FunctionCode.AtomicRmw, [Relative(operands[0]), Relative(operands[1]), (ulong)instruction.Code, 0, ModuleWriter.Records.Ordering, ModuleWriter.Records.Scope]);
                break;
            case Opcode.CompareExchange:
                Record(FunctionCode.CompareExchange, [Relative(operands[0]), Relative(operands[1]), Relative(operands[ThirdOperand]), 0, ModuleWriter.Records.Ordering, ModuleWriter.Records.Scope, ModuleWriter.Records.Ordering, 0]);
                break;
            case Opcode.GetElementPtr:
                Record(FunctionCode.GetElementPtr, [1, (ulong)instruction.SourceType!.Id, .. operands.Select(Relative)]);
                break;
            case Opcode.Alloca:
                Record(FunctionCode.Alloca, [(ulong)instruction.SourceType!.Id, (ulong)_types.Int32.Id, (ulong)operands[0].Id, ModuleWriter.Records.Alignment | ModuleWriter.Records.AllocaFlags]);
                break;
            case Opcode.Call:
                WriteCall(instruction);
                break;
            case Opcode.Phi:
                WritePhi(instruction);
                break;
            default:
                WriteTerminator(instruction);
                break;
        }
    }

    private void WriteCall(Instruction call)
    {
        var callee = call.Callee!;
        var operands = call.Operands;
        Span<ulong> record = stackalloc ulong[CallFields + operands.Count];

        record[0] = 0;
        record[1] = ModuleWriter.Records.ExplicitType;
        record[ThirdOperand] = (ulong)callee.FunctionType.Id;
        record[ThirdOperand + 1] = Relative(callee);

        for (var index = 0; index < operands.Count; index++)
        {
            record[CallFields + index] = Relative(operands[index]);
        }

        Record(FunctionCode.Call, record);
    }

    private void WritePhi(Instruction phi)
    {
        var record = new List<ulong> { (ulong)phi.Type.Id };

        for (var index = 0; index < phi.Operands.Count; index++)
        {
            var position = phi.Targets[index].Position;

            if (position < 0)
            {
                continue;
            }

            if (phi.Operands[index].Id < 0)
            {
                throw new InvalidOperationException($"a phi operand that is not written: {phi.Operands[index]}");
            }

            record.Add(BitstreamWriter.Signed((long)_next - phi.Operands[index].Id));
            record.Add((ulong)position);
        }

        Record(FunctionCode.Phi, [.. record]);
    }

    private void WriteTerminator(Instruction terminator)
    {
        var targets = terminator.Targets.Select(target => (ulong)target.Position).ToArray();

        switch (terminator.Opcode)
        {
            case Opcode.Return:
                Record(FunctionCode.Return, []);
                break;
            case Opcode.Branch:
                Record(FunctionCode.Branch, [targets[0]]);
                break;
            case Opcode.ConditionalBranch:
                Record(FunctionCode.Branch, [targets[0], targets[1], Relative(terminator.Operands[0])]);
                break;
            default:
                WriteSwitch(terminator, targets);
                break;
        }
    }

    private void WriteSwitch(Instruction terminator, ulong[] targets)
    {
        var condition = terminator.Operands[0];
        var record = new List<ulong> { (ulong)condition.Type.Id, Relative(condition), targets[0] };

        for (var index = 0; index < terminator.Cases.Count; index++)
        {
            record.Add((ulong)terminator.Cases[index].Id);
            record.Add(targets[index + 1]);
        }

        Record(FunctionCode.Switch, [.. record]);
    }

    private enum FunctionCode
    {
        DeclareBlocks = 1,
        Binary = 2,
        Cast = 3,
        Return = 10,
        Branch = 11,
        Switch = 12,
        Phi = 16,
        Alloca = 19,
        Load = 20,
        ExtractValue = 26,
        Compare = 28,
        Select = 29,
        Call = 34,
        AtomicRmw = 38,
        GetElementPtr = 43,
        Store = 44,
        CompareExchange = 46,
    }
}
