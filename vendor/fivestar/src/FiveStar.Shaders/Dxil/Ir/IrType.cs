// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Dxil.Ir;

/// <summary>
/// An LLVM type, interned by <see cref="TypeTable"/>: its number is its index in the module's type table, and every type
/// it refers to has a lower number.
/// </summary>
public sealed class IrType
{
    internal IrType(int id, IrTypeKind kind)
    {
        Id = id;
        Kind = kind;
    }

    public int Id { get; }

    public IrTypeKind Kind { get; }

    /// <summary>Integer width.</summary>
    public int Bits { get; init; }

    /// <summary>Pointer pointee, array element or function return type.</summary>
    public IrType? Element { get; init; }

    /// <summary>Array length.</summary>
    public int Count { get; init; }

    public int AddressSpace { get; init; }

    /// <summary>Struct fields or function parameters.</summary>
    public IReadOnlyList<IrType> Members { get; init; } = [];

    /// <summary>A named struct's name.</summary>
    public string? Name { get; init; }

    public bool IsVoid => Kind == IrTypeKind.Void;

    public bool IsFloatingPoint => Kind is IrTypeKind.Half or IrTypeKind.Float or IrTypeKind.Double;

    public bool IsInteger => Kind == IrTypeKind.Integer;

    public override string ToString()
    {
        return Kind switch
        {
            IrTypeKind.Integer => $"i{Bits}",
            IrTypeKind.Pointer => $"{Element}*",
            IrTypeKind.Array => $"[{Count} x {Element}]",
            IrTypeKind.Struct => Name ?? "{...}",
            _ => Kind.ToString().ToLowerInvariant(),
        };
    }
}
