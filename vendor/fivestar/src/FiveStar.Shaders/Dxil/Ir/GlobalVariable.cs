// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Dxil.Ir;

/// <summary>
/// A global variable: thread-group shared memory (address space 3, external) or an internal constant table. Its value is
/// a pointer to <see cref="ValueType"/>.
/// </summary>
public sealed class GlobalVariable : Value
{
    internal GlobalVariable(string name, IrType valueType, IrType pointerType, Constant? initializer)
        : base(pointerType)
    {
        Name = name;
        ValueType = valueType;
        Initializer = initializer;
    }

    public string Name { get; }

    public IrType ValueType { get; }

    public int AddressSpace => Type.AddressSpace;

    /// <summary>An internal constant's contents; null for shared memory.</summary>
    public Constant? Initializer { get; }
}
