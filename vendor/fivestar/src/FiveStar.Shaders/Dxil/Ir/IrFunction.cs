// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Dxil.Ir;

/// <summary>A function: a <c>dx.op</c> declaration or the entry point, which has blocks. Its value is a function pointer.</summary>
public sealed class IrFunction : Value
{
    internal IrFunction(string name, IrType functionType, IrType pointerType, AttributeSet attribute, bool hasBody)
        : base(pointerType)
    {
        Name = name;
        FunctionType = functionType;
        Attribute = attribute;
        HasBody = hasBody;
    }

    public string Name { get; }

    public IrType FunctionType { get; }

    public AttributeSet Attribute { get; set; }

    public bool HasBody { get; }

    /// <summary>The body; the first block is the entry.</summary>
    public List<BasicBlock> Blocks { get; } = [];
}
