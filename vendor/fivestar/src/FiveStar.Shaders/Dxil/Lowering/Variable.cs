// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Dxil.Ir;

namespace FiveStar.Shaders.Dxil.Lowering;

/// <summary>
/// A declared variable. Slots: <see cref="FirstSlot"/> and the following ones, one per component. Memory:
/// <see cref="Pointer"/> to an array of <see cref="ElementType"/> holding the components in order. Cbuffer: the byte
/// offset of the member.
/// </summary>
public sealed class Variable
{
    public Variable(string name, HlslType type, StorageKind storage)
    {
        Name = name;
        Type = type;
        Storage = storage;
    }

    public string Name { get; }

    public HlslType Type { get; }

    public StorageKind Storage { get; }

    public int FirstSlot { get; init; }

    public Value? Pointer { get; init; }

    /// <summary>The memory element type: i32 (32-bit components, floats as their bits) or i64.</summary>
    public IrType? ElementType { get; init; }

    public int CbufferOffset { get; init; }

    /// <summary>A read-only constant table (stores are refused).</summary>
    public bool IsConstant { get; init; }

    /// <summary>A static const scalar or vector: its constant components (reads use them directly, also as case labels).</summary>
    public IReadOnlyList<Value>? Fixed { get; set; }

    /// <summary>The whole variable as a place, made once.</summary>
    internal LValue? WholePlace { get; set; }

    public override string ToString()
    {
        return $"{Type} {Name}";
    }
}
