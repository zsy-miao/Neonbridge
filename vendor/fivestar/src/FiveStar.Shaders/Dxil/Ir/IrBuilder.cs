// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Dxil.Ir;

/// <summary>Appends instructions to the current block of a module's entry function.</summary>
public sealed class IrBuilder
{
    private const string DxOpPrefix = "dx.op.";
    private const int BinaryOperands = 2;
    private const int TernaryOperands = 3;
    private const long SignatureBasis = unchecked((long)0xcbf2_9ce4_8422_2325);
    private const long SignaturePrime = 0x100_0000_01b3;

    // dx.op declarations by name and type signature, so a call does not rebuild the type key and name.
    private readonly Dictionary<(string Name, int Result, long Signature), IrFunction> _operations = [];

    public IrBuilder(DxilModule module, BasicBlock entry)
    {
        Module = module;
        Block = entry;
    }

    public DxilModule Module { get; }

    public TypeTable Types => Module.Types;

    /// <summary>The block instructions go to.</summary>
    public BasicBlock Block { get; set; }

    /// <summary>Whether results known while lowering are written as constants or operands (<see cref="ConstantFolding"/>).</summary>
    public bool Fold { get; init; } = true;

    public Value Binary(BinaryOp op, Value left, Value right)
    {
        var folded = Fold ? ConstantFolding.Binary(Module, op, left, right) : null;

        if (folded is not null)
        {
            return folded;
        }

        return Append(new Instruction(Opcode.Binary, left.Type, Block, BinaryOperands) { Code = (int)op }, left, right);
    }

    public Value Cast(CastOp op, Value value, IrType to)
    {
        var folded = Fold ? ConstantFolding.Cast(Module, op, value, to) : null;

        if (folded is not null)
        {
            return folded;
        }

        return Append(new Instruction(Opcode.Cast, to, Block, 1) { Code = (int)op }, value);
    }

    public Value Compare(Predicate predicate, Value left, Value right)
    {
        var folded = Fold ? ConstantFolding.Compare(Module, predicate, left, right) : null;

        if (folded is not null)
        {
            return folded;
        }

        return Append(new Instruction(Opcode.Compare, Types.Bool, Block, BinaryOperands) { Code = (int)predicate }, left, right);
    }

    public Value Select(Value condition, Value whenTrue, Value whenFalse)
    {
        var folded = Fold ? ConstantFolding.Select(condition, whenTrue, whenFalse) : null;

        if (folded is not null)
        {
            return folded;
        }

        return Append(new Instruction(Opcode.Select, whenTrue.Type, Block, TernaryOperands), condition, whenTrue, whenFalse);
    }

    public Value Extract(Value aggregate, int index)
    {
        var type = aggregate.Type.Kind == IrTypeKind.Array ? aggregate.Type.Element! : aggregate.Type.Members[index];

        return Append(new Instruction(Opcode.ExtractValue, type, Block, 1) { Code = index }, aggregate);
    }

    public Value Call(IrFunction function, params Value[] arguments)
    {
        return Append(new Instruction(Opcode.Call, function.FunctionType.Element!, Block) { Callee = function }, arguments);
    }

    /// <summary>
    /// Calls <c>dx.op.&lt;name&gt;</c> with <paramref name="opcode"/> first; the declaration takes the argument types.
    /// </summary>
    public Value DxOp(string name, int opcode, AttributeSet attribute, IrType result, params Value[] arguments)
    {
        var signature = SignatureBasis;

        foreach (var argument in arguments)
        {
            signature = unchecked((signature ^ argument.Type.Id) * SignaturePrime);
        }

        if (!_operations.TryGetValue((name, result.Id, signature), out var declared))
        {
            declared = Declare(name, attribute, result, arguments);
            _operations[(name, result.Id, signature)] = declared;
        }

        var all = new Value[arguments.Length + 1];
        all[0] = Module.Int32((uint)opcode);
        arguments.CopyTo(all, 1);

        return Call(declared, all);
    }

    private IrFunction Declare(string name, AttributeSet attribute, IrType result, Value[] arguments)
    {
        var parameters = new IrType[arguments.Length + 1];
        parameters[0] = Types.Int32;

        for (var index = 0; index < arguments.Length; index++)
        {
            parameters[index + 1] = arguments[index].Type;
        }

        return Module.Function(DxOpPrefix + name, Types.Function(result, parameters), attribute, hasBody: false);
    }

    /// <summary>A phi at the top of <paramref name="block"/>; its incoming values are added later.</summary>
    public static Instruction Phi(BasicBlock block, IrType type)
    {
        var phi = new Instruction(Opcode.Phi, type, block);
        block.Phis.Add(phi);

        return phi;
    }

    /// <summary>The address of element <paramref name="index"/> of the array <paramref name="pointer"/> points to.</summary>
    public Value Element(Value pointer, Value index)
    {
        var array = pointer.Type.Element!;
        var type = Types.Pointer(array.Element!, pointer.Type.AddressSpace);

        return Append(new Instruction(Opcode.GetElementPtr, type, Block, TernaryOperands) { SourceType = array }, pointer, Module.Int32(0), index);
    }

    public Value Load(Value pointer)
    {
        return Append(new Instruction(Opcode.Load, pointer.Type.Element!, Block, 1), pointer);
    }

    public void Store(Value pointer, Value value)
    {
        Append(new Instruction(Opcode.Store, Types.Void, Block, BinaryOperands), pointer, value);
    }

    public Value AtomicRmw(AtomicOp op, Value pointer, Value value)
    {
        return Append(new Instruction(Opcode.AtomicRmw, value.Type, Block, BinaryOperands) { Code = (int)op }, pointer, value);
    }

    /// <summary>The old value of a compare-exchange.</summary>
    public Value CompareExchange(Value pointer, Value expected, Value replacement)
    {
        var pair = Types.Struct(null, [replacement.Type, Types.Bool]);
        var result = Append(new Instruction(Opcode.CompareExchange, pair, Block, TernaryOperands), pointer, expected, replacement);

        return Extract(result, 0);
    }

    /// <summary>A local array in the entry block (allocas belong there).</summary>
    public Value Alloca(BasicBlock entry, IrType type)
    {
        var alloca = new Instruction(Opcode.Alloca, Types.Pointer(type, 0), entry) { SourceType = type };
        alloca.Operands.Add(Module.Int32(1));
        entry.Body.Insert(0, alloca);

        return alloca;
    }

    public void Branch(BasicBlock target)
    {
        var branch = new Instruction(Opcode.Branch, Types.Void, Block);
        branch.Targets.Add(target);
        Block.Terminator = branch;
    }

    public void ConditionalBranch(Value condition, BasicBlock whenTrue, BasicBlock whenFalse)
    {
        var branch = new Instruction(Opcode.ConditionalBranch, Types.Void, Block);
        branch.Operands.Add(condition);
        branch.Targets.Add(whenTrue);
        branch.Targets.Add(whenFalse);
        Block.Terminator = branch;
    }

    public void Switch(Value condition, BasicBlock fallback, IReadOnlyList<(Constant Value, BasicBlock Target)> cases)
    {
        var instruction = new Instruction(Opcode.Switch, Types.Void, Block);
        instruction.Operands.Add(condition);
        instruction.Targets.Add(fallback);

        foreach (var (value, target) in cases)
        {
            instruction.Cases.Add(value);
            instruction.Targets.Add(target);
        }

        Block.Terminator = instruction;
    }

    public void Return()
    {
        Block.Terminator = new Instruction(Opcode.Return, Types.Void, Block);
    }

    private Instruction Append(Instruction instruction, Value first)
    {
        instruction.Operands.Add(first);

        return Append(instruction);
    }

    private Instruction Append(Instruction instruction, Value first, Value second)
    {
        instruction.Operands.Add(first);
        instruction.Operands.Add(second);

        return Append(instruction);
    }

    private Instruction Append(Instruction instruction, Value first, Value second, Value third)
    {
        instruction.Operands.Add(first);
        instruction.Operands.Add(second);
        instruction.Operands.Add(third);

        return Append(instruction);
    }

    private Instruction Append(Instruction instruction, Value[] operands)
    {
        instruction.Operands.AddRange(operands);

        return Append(instruction);
    }

    private Instruction Append(Instruction instruction)
    {
        Block.Body.Add(instruction);

        return instruction;
    }
}
