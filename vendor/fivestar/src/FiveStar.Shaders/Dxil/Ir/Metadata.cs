// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Dxil.Ir;

/// <summary>A metadata string, a constant or function as metadata, or a node of other metadata (null operands allowed).</summary>
public sealed class Metadata
{
    internal Metadata(int id, string? text, Value? value, IReadOnlyList<Metadata?> operands)
    {
        Id = id;
        Text = text;
        Value = value;
        Operands = operands;
    }

    /// <summary>The number in the module's metadata list (creation order: operands come first).</summary>
    public int Id { get; }

    public string? Text { get; }

    public Value? Value { get; }

    public IReadOnlyList<Metadata?> Operands { get; }
}
