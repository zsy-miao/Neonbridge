// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Dxil.Bitcode;

namespace FiveStar.Shaders.Dxil.Ir;

/// <summary>
/// Writes a <see cref="DxilModule"/> as LLVM 3.7 bitcode with unabbreviated records (codes from LLVMBitCodes.h). Value
/// numbers: globals, functions, constants, then the entry function's instructions in block order. Blocks are laid out
/// in reverse postorder (unreachable ones are left out), and an instruction without effect that nothing reads is not
/// written.
/// </summary>
public sealed class ModuleWriter
{
    private const string Triple = "dxil-ms-dx";
    private const string DataLayout = "e-m:e-p:32:32-i1:32-i8:32-i16:32-i32:32-i64:64-f16:32-f32:32-f64:64-n8:16:32:64";
    private const int ModuleWidth = 3;
    private const int WideWidth = 4;
    private const ulong FunctionAttributeIndex = 0xFFFF_FFFF;
    private const ulong ExplicitTypeFlag = 1 << 15;
    private const ulong ExplicitGlobalType = 2;
    private const ulong InternalLinkage = 3;
    private const ulong Align4 = 3;
    private const ulong AllocaExplicitType = 1 << 6;
    private const ulong SequentiallyConsistent = 6;
    private const ulong CrossThread = 1;
    private const int AddressSpaceShift = 2;

    private static readonly byte[] MagicTail = [0xC0, 0xDE];

    private static readonly string[] MetadataKinds =
    [
        "dbg", "tbaa", "prof", "fpmath", "range", "tbaa.struct", "invariant.load", "alias.scope", "noalias", "nontemporal",
        "llvm.mem.parallel_loop_access", "nonnull", "dereferenceable", "dereferenceable_or_null",
    ];

    // Attribute kinds: 12 noduplicate, 18 nounwind, 20 readnone, 21 readonly; one group and one list per AttributeSet.
    private static readonly ulong[][] AttributeGroups = [[18, 20], [18], [18, 21], [12, 18], []];

    // String attributes (key only) per group.
    private static readonly string?[] AttributeStrings = [null, null, null, null, "waveops-include-helper-lanes"];
    private const ulong StringAttribute = 3;

    private readonly DxilModule _module;
    private readonly BitstreamWriter _out = new();
    private int _moduleValues;
    private List<GlobalVariable> _globals = [];
    private List<IrFunction> _functions = [];

    private ModuleWriter(DxilModule module)
    {
        _module = module;
    }

    public static byte[] Write(DxilModule module)
    {
        var writer = new ModuleWriter(module);
        writer.WriteModule();

        return writer._out.ToArray();
    }

    private void WriteModule()
    {
        _out.Bytes("BC"u8);
        _out.Bytes(MagicTail);
        _out.Enter(BlockId.Module, ModuleWidth);
        _out.Record((int)ModuleCode.Version, [1]);
        WriteAttributes();
        WriteTypes();

        _out.TextRecord((int)ModuleCode.Triple, [], Triple);
        _out.TextRecord((int)ModuleCode.DataLayout, [], DataLayout);

        var entry = _module.Functions.First(function => function.HasBody);
        var body = new FunctionBodyWriter(_out, entry, _module.Types);
        body.Analyze();
        var referenced = body.Referenced();
        _globals = [.. _module.Globals.Where(referenced.Contains)];
        _functions = [.. _module.Functions.Where(function => function == entry || referenced.Contains(function))];

        NumberModuleValues();
        WriteGlobals();
        WriteFunctionDeclarations();
        WriteConstants();
        body.NumberInstructions(_moduleValues);
        WriteMetadata();

        WriteSymbols();
        body.Write();
        _out.Exit();
    }

    private void WriteAttributes()
    {
        _out.Enter(BlockId.ParameterAttributeGroups, ModuleWidth);

        for (var group = 0; group < AttributeGroups.Length; group++)
        {
            var record = new List<ulong> { (ulong)group + 1, FunctionAttributeIndex };

            foreach (var kind in AttributeGroups[group])
            {
                record.Add(0);
                record.Add(kind);
            }

            var text = AttributeStrings[group];

            if (text is not null)
            {
                record.Add(StringAttribute);
                record.AddRange(text.Select(character => (ulong)character));
                record.Add(0);
            }

            _out.Record((int)AttributeCode.GroupEntry, [.. record]);
        }

        _out.Exit();
        _out.Enter(BlockId.ParameterAttributes, ModuleWidth);

        for (var group = 1; group <= AttributeGroups.Length; group++)
        {
            _out.Record((int)AttributeCode.Entry, [(ulong)group]);
        }

        _out.Exit();
    }

    private void WriteTypes()
    {
        _out.Enter(BlockId.Types, WideWidth);
        _out.Record((int)TypeCode.EntryCount, [(ulong)_module.Types.All.Count]);

        foreach (var type in _module.Types.All)
        {
            WriteType(type);
        }

        _out.Exit();
    }

    private void WriteType(IrType type)
    {
        var members = type.Members.Select(member => (ulong)member.Id);

        switch (type.Kind)
        {
            case IrTypeKind.Void:
                _out.Record((int)TypeCode.Void, []);
                break;
            case IrTypeKind.Half:
                _out.Record((int)TypeCode.Half, []);
                break;
            case IrTypeKind.Float:
                _out.Record((int)TypeCode.Float, []);
                break;
            case IrTypeKind.Double:
                _out.Record((int)TypeCode.Double, []);
                break;
            case IrTypeKind.Integer:
                _out.Record((int)TypeCode.Integer, [(ulong)type.Bits]);
                break;
            case IrTypeKind.Pointer:
                _out.Record((int)TypeCode.Pointer, [(ulong)type.Element!.Id, (ulong)type.AddressSpace]);
                break;
            case IrTypeKind.Array:
                _out.Record((int)TypeCode.Array, [(ulong)type.Count, (ulong)type.Element!.Id]);
                break;
            case IrTypeKind.Function:
                _out.Record((int)TypeCode.Function, [0, (ulong)type.Element!.Id, .. members]);
                break;
            default:
                WriteStruct(type, members);
                break;
        }
    }

    private void WriteStruct(IrType type, IEnumerable<ulong> members)
    {
        if (type.Name is null)
        {
            _out.Record((int)TypeCode.StructAnonymous, [0, .. members]);

            return;
        }

        _out.TextRecord((int)TypeCode.StructName, [], type.Name);
        _out.Record((int)TypeCode.StructNamed, [0, .. members]);
    }

    private void NumberModuleValues()
    {
        foreach (var value in _globals.Cast<Value>().Concat(_functions).Concat(_module.Constants))
        {
            value.Id = _moduleValues;
            _moduleValues++;
        }
    }

    private void WriteGlobals()
    {
        foreach (var global in _globals)
        {
            ulong constant = 0;
            ulong initializer = 0;
            ulong linkage = 0;

            if (global.Initializer is not null)
            {
                constant = 1;
                initializer = (ulong)global.Initializer.Id + 1;
                linkage = InternalLinkage;
            }

            var flags = ((ulong)global.AddressSpace << AddressSpaceShift) | ExplicitGlobalType | constant;
            _out.Record((int)ModuleCode.GlobalVariable, [(ulong)global.ValueType.Id, flags, initializer, linkage, Align4, 0]);
        }
    }

    private void WriteFunctionDeclarations()
    {
        foreach (var function in _functions)
        {
            ulong prototype = function.HasBody ? 0UL : 1UL;
            _out.Record((int)ModuleCode.Function, [(ulong)function.FunctionType.Id, 0, prototype, 0, (ulong)function.Attribute, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        }
    }

    private void WriteConstants()
    {
        _out.Enter(BlockId.Constants, WideWidth);
        IrType? current = null;

        foreach (var constant in _module.Constants)
        {
            if (current != constant.Type)
            {
                _out.Record((int)ConstantCode.SetType, [(ulong)constant.Type.Id]);
                current = constant.Type;
            }

            WriteConstant(constant);
        }

        _out.Exit();
    }

    private void WriteConstant(Constant constant)
    {
        switch (constant.Kind)
        {
            case ConstantKind.Undefined:
                _out.Record((int)ConstantCode.Undefined, []);
                break;
            case ConstantKind.Null:
                _out.Record((int)ConstantCode.Null, []);
                break;
            case ConstantKind.Integer:
                _out.Record((int)ConstantCode.Integer, [BitstreamWriter.Signed(constant.Bits)]);
                break;
            case ConstantKind.FloatingPoint:
                _out.Record((int)ConstantCode.Float, [(ulong)constant.Bits]);
                break;
            default:
                _out.Record((int)ConstantCode.Aggregate, [.. constant.Elements.Select(element => (ulong)element.Id)]);
                break;
        }
    }

    private void WriteMetadata()
    {
        _out.Enter(BlockId.Metadata, ModuleWidth);

        foreach (var metadata in _module.MetadataList)
        {
            if (metadata.Text is not null)
            {
                _out.TextRecord((int)MetadataCode.String, [], metadata.Text);
            }
            else if (metadata.Value is not null)
            {
                _out.Record((int)MetadataCode.Value, [(ulong)metadata.Value.Type.Id, (ulong)metadata.Value.Id]);
            }
            else
            {
                _out.Record((int)MetadataCode.Node, [.. metadata.Operands.Select(OperandNumber)]);
            }
        }

        foreach (var (name, nodes) in _module.NamedMetadata)
        {
            _out.TextRecord((int)MetadataCode.Name, [], name);
            _out.Record((int)MetadataCode.NamedNode, [.. nodes.Select(node => (ulong)node.Id)]);
        }

        _out.Exit();
        _out.Enter(BlockId.Metadata, ModuleWidth);

        for (var kind = 0; kind < MetadataKinds.Length; kind++)
        {
            _out.TextRecord((int)MetadataCode.Kind, [(ulong)kind], MetadataKinds[kind]);
        }

        _out.Exit();
    }

    /// <summary>A node operand: the metadata number + 1, or 0 for none.</summary>
    private static ulong OperandNumber(Metadata? operand)
    {
        if (operand is null)
        {
            return 0;
        }

        return (ulong)operand.Id + 1;
    }

    private void WriteSymbols()
    {
        _out.Enter(BlockId.ValueSymbolTable, WideWidth);

        foreach (var global in _globals)
        {
            _out.TextRecord((int)SymbolCode.Entry, [(ulong)global.Id], global.Name);
        }

        foreach (var function in _functions)
        {
            _out.TextRecord((int)SymbolCode.Entry, [(ulong)function.Id], function.Name);
        }

        _out.Exit();
    }

    private enum ModuleCode
    {
        Version = 1,
        Triple = 2,
        DataLayout = 3,
        GlobalVariable = 7,
        Function = 8,
    }

    private enum AttributeCode
    {
        Entry = 2,
        GroupEntry = 3,
    }

    private enum TypeCode
    {
        EntryCount = 1,
        Void = 2,
        Float = 3,
        Double = 4,
        Integer = 7,
        Pointer = 8,
        Half = 10,
        Array = 11,
        StructAnonymous = 18,
        StructName = 19,
        StructNamed = 20,
        Function = 21,
    }

    private enum ConstantCode
    {
        SetType = 1,
        Null = 2,
        Undefined = 3,
        Integer = 4,
        Float = 6,
        Aggregate = 7,
    }

    private enum MetadataCode
    {
        String = 1,
        Value = 2,
        Node = 3,
        Name = 4,
        Kind = 6,
        NamedNode = 10,
    }

    private enum SymbolCode
    {
        Entry = 1,
    }

    /// <summary>Record constants shared with <see cref="FunctionBodyWriter"/>.</summary>
    internal static class Records
    {
        public const ulong ExplicitType = ExplicitTypeFlag;
        public const ulong Alignment = Align4;
        public const ulong AllocaFlags = AllocaExplicitType;
        public const ulong Ordering = SequentiallyConsistent;
        public const ulong Scope = CrossThread;
    }
}
