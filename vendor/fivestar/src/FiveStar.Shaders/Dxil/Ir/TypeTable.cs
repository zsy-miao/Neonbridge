// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Dxil.Ir;

/// <summary>The module's types, each made once: asking for the same type again returns the same object.</summary>
public sealed class TypeTable
{
    private const int BoolBits = 1;
    private const int ByteBits = 8;
    private const int WordBits = 32;
    private const int LongBits = 64;

    private readonly List<IrType> _types = [];
    private readonly Dictionary<string, IrType> _byKey = new(StringComparer.Ordinal);

    public TypeTable()
    {
        Void = Make("v", IrTypeKind.Void, static (id, kind) => new IrType(id, kind));
        Bool = Integer(BoolBits);
        Int8 = Integer(ByteBits);
        Int32 = Integer(WordBits);
        Int64 = Integer(LongBits);
        Float = Make("f", IrTypeKind.Float, static (id, kind) => new IrType(id, kind));
    }

    public IReadOnlyList<IrType> All => _types;

    public IrType Void { get; }

    public IrType Bool { get; }

    public IrType Int8 { get; }

    public IrType Int32 { get; }

    public IrType Int64 { get; }

    public IrType Float { get; }

    public IrType Half => Make("h", IrTypeKind.Half, static (id, kind) => new IrType(id, kind));

    public IrType Double => Make("d", IrTypeKind.Double, static (id, kind) => new IrType(id, kind));

    public IrType Integer(int bits)
    {
        return Make($"i{bits}", IrTypeKind.Integer, (id, kind) => new IrType(id, kind) { Bits = bits });
    }

    public IrType Pointer(IrType pointee, int addressSpace)
    {
        return Make($"p{pointee.Id}:{addressSpace}", IrTypeKind.Pointer, (id, kind) => new IrType(id, kind) { Element = pointee, AddressSpace = addressSpace });
    }

    public IrType Array(int count, IrType element)
    {
        return Make($"a{count}x{element.Id}", IrTypeKind.Array, (id, kind) => new IrType(id, kind) { Element = element, Count = count });
    }

    /// <summary>A named struct (<paramref name="name"/> non-null) or a literal one.</summary>
    public IrType Struct(string? name, IReadOnlyList<IrType> fields)
    {
        var key = $"s{name}|{string.Join(',', fields.Select(field => field.Id))}";

        return Make(key, IrTypeKind.Struct, (id, kind) => new IrType(id, kind) { Name = name, Members = [.. fields] });
    }

    public IrType Function(IrType result, IReadOnlyList<IrType> parameters)
    {
        var key = $"F{result.Id}({string.Join(',', parameters.Select(parameter => parameter.Id))})";

        return Make(key, IrTypeKind.Function, (id, kind) => new IrType(id, kind) { Element = result, Members = [.. parameters] });
    }

    private IrType Make(string key, IrTypeKind kind, Func<int, IrTypeKind, IrType> create)
    {
        if (_byKey.TryGetValue(key, out var existing))
        {
            return existing;
        }

        var type = create(_types.Count, kind);
        _types.Add(type);
        _byKey[key] = type;

        return type;
    }
}
