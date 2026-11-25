// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Dxil.Ir;

/// <summary>
/// A DXIL module under construction: types, constants (each made once), globals, the <c>dx.op</c> declarations and the
/// entry function, and the metadata. <see cref="ModuleWriter"/> turns it into LLVM 3.7 bitcode.
/// </summary>
public sealed class DxilModule
{
    private const int BoolBits = 1;

    private readonly Dictionary<(int Type, ConstantKind Kind, long Bits, string Elements), Constant> _constantIndex = [];
    private readonly Dictionary<(int Type, ConstantKind Kind, long Bits), Constant> _scalarIndex = [];
    private readonly Dictionary<string, IrFunction> _functionIndex = new(StringComparer.Ordinal);
    private readonly List<Constant> _constants = [];
    private readonly Dictionary<Constant, int> _constantNumbers = [];
    private readonly List<GlobalVariable> _globals = [];
    private readonly List<IrFunction> _functions = [];
    private readonly List<Metadata> _metadata = [];
    private readonly List<(string Name, IReadOnlyList<Metadata> Nodes)> _named = [];
    private int _blocks;

    public TypeTable Types { get; } = new();

    public IReadOnlyList<Constant> Constants => _constants;

    public IReadOnlyList<GlobalVariable> Globals => _globals;

    public IReadOnlyList<IrFunction> Functions => _functions;

    public IReadOnlyList<Metadata> MetadataList => _metadata;

    public IReadOnlyList<(string Name, IReadOnlyList<Metadata> Nodes)> NamedMetadata => _named;

    public Constant Int(IrType type, long value)
    {
        if (!type.IsInteger)
        {
            throw new InvalidOperationException($"an integer constant of type {type}");
        }

        var bits = SignExtend(value, type.Bits);

        return Intern(type, ConstantKind.Integer, bits, []);
    }

    public Constant Int32(uint value)
    {
        return Int(Types.Int32, (int)value);
    }

    public Constant Int8(byte value)
    {
        return Int(Types.Int8, value);
    }

    public Constant Bool(bool value)
    {
        long bit = value ? 1 : 0;

        return Int(Types.Bool, bit);
    }

    public Constant Float(float value)
    {
        return Intern(Types.Float, ConstantKind.FloatingPoint, BitConverter.SingleToUInt32Bits(value), []);
    }

    /// <summary>A floating-point constant of <paramref name="type"/> from its bits (half, float or double layout).</summary>
    public Constant FloatBits(IrType type, long bits)
    {
        if (!type.IsFloatingPoint)
        {
            throw new InvalidOperationException($"a floating-point constant of type {type}");
        }

        return Intern(type, ConstantKind.FloatingPoint, bits, []);
    }

    public Constant Undefined(IrType type)
    {
        return Intern(type, ConstantKind.Undefined, 0, []);
    }

    public Constant Null(IrType type)
    {
        return Intern(type, ConstantKind.Null, 0, []);
    }

    public Constant Aggregate(IrType type, IReadOnlyList<Constant> elements)
    {
        return Intern(type, ConstantKind.Aggregate, 0, elements);
    }

    /// <summary>A declared function, found by name; the first request decides its type and attribute.</summary>
    public IrFunction Function(string name, IrType functionType, AttributeSet attribute, bool hasBody)
    {
        if (_functionIndex.TryGetValue(name, out var existing))
        {
            return existing;
        }

        var function = new IrFunction(name, functionType, Types.Pointer(functionType, 0), attribute, hasBody);
        _functions.Add(function);
        _functionIndex[name] = function;

        return function;
    }

    public GlobalVariable Global(string name, IrType valueType, int addressSpace, Constant? initializer)
    {
        var global = new GlobalVariable(name, valueType, Types.Pointer(valueType, addressSpace), initializer);
        _globals.Add(global);

        return global;
    }

    public BasicBlock NewBlock()
    {
        _blocks++;

        return new BasicBlock(_blocks);
    }

    public Metadata Text(string text)
    {
        return AddMetadata(text, null, []);
    }

    public Metadata ValueMetadata(Value value)
    {
        return AddMetadata(null, value, []);
    }

    public Metadata Node(params Metadata?[] operands)
    {
        return AddMetadata(null, null, operands);
    }

    public Metadata Int32Metadata(uint value)
    {
        return ValueMetadata(Int32(value));
    }

    public void Named(string name, params Metadata[] nodes)
    {
        _named.Add((name, nodes));
    }

    private Metadata AddMetadata(string? text, Value? value, IReadOnlyList<Metadata?> operands)
    {
        var metadata = new Metadata(_metadata.Count, text, value, operands);
        _metadata.Add(metadata);

        return metadata;
    }

    private Constant Intern(IrType type, ConstantKind kind, long bits, IReadOnlyList<Constant> elements)
    {
        if (elements.Count == 0)
        {
            return InternScalar(type, kind, bits);
        }

        var key = (type.Id, kind, bits, string.Join(',', elements.Select(element => _constantNumbers[element])));

        if (_constantIndex.TryGetValue(key, out var existing))
        {
            return existing;
        }

        var constant = new Constant(type, kind, bits, elements);
        _constantNumbers[constant] = _constants.Count;
        _constants.Add(constant);
        _constantIndex[key] = constant;

        return constant;
    }

    private Constant InternScalar(IrType type, ConstantKind kind, long bits)
    {
        if (_scalarIndex.TryGetValue((type.Id, kind, bits), out var existing))
        {
            return existing;
        }

        var constant = new Constant(type, kind, bits, []);
        _constantNumbers[constant] = _constants.Count;
        _constants.Add(constant);
        _scalarIndex[(type.Id, kind, bits)] = constant;

        return constant;
    }

    private static long SignExtend(long value, int bits)
    {
        if (bits == BoolBits)
        {
            return value & 1;
        }

        var shift = (sizeof(long) * BitsPerByte) - bits;

        return (value << shift) >> shift;
    }

    private const int BitsPerByte = 8;
}
