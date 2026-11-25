// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Dxil.Hlsl;

/// <summary>
/// A written type: its name (<c>uint4</c>, a struct, <c>Texture2D</c>), template arguments (<c>&lt;float4&gt;</c>,
/// <c>vector&lt;float, 4&gt;</c>: a type, then integer sizes) and array dimensions (null: unsized).
/// </summary>
public sealed class TypeSyntax
{
    public TypeSyntax(string name)
    {
        Name = name;
    }

    public string Name { get; }

    public TypeSyntax? Argument { get; init; }

    public IReadOnlyList<long> Sizes { get; init; } = [];

    public List<SyntaxNode?> Dimensions { get; } = [];

    /// <summary>The same type with these array dimensions.</summary>
    public TypeSyntax WithDimensions(IEnumerable<SyntaxNode?> dimensions)
    {
        var copy = new TypeSyntax(Name) { Argument = Argument, Sizes = Sizes };
        copy.Dimensions.AddRange(Dimensions);
        copy.Dimensions.AddRange(dimensions);

        return copy;
    }

    public override string ToString()
    {
        return Name;
    }
}
