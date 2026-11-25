// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Dxil.Hlsl;

namespace FiveStar.Shaders.Dxil.Lowering;

/// <summary>Turns written types into <see cref="HlslType"/>s: scalars, vectors, matrices, arrays, declared structs, resources.</summary>
public sealed class TypeResolver
{
    private readonly Dictionary<string, HlslType> _structs = new(StringComparer.Ordinal);

    public void AddStruct(string name, HlslType type)
    {
        _structs[name] = type;
    }

    public bool IsStruct(string name)
    {
        return _structs.ContainsKey(name);
    }

    /// <summary>The type of <paramref name="syntax"/>; array sizes must be constant (<paramref name="size"/> evaluates them).</summary>
    public HlslType Resolve(TypeSyntax syntax, Func<SyntaxNode?, int> size)
    {
        var type = Base(syntax);

        for (var index = syntax.Dimensions.Count - 1; index >= 0; index--)
        {
            type = HlslType.Array(type, size(syntax.Dimensions[index]));
        }

        return type;
    }

    public HlslType Base(TypeSyntax syntax)
    {
        var name = syntax.Name;

        if (_structs.TryGetValue(name, out var structure))
        {
            return structure;
        }

        if (TypeNames.IsScalar(name))
        {
            return HlslType.ScalarOf(Kind(name));
        }

        var shape = TypeNames.Shape(name);

        if (shape is not null)
        {
            var (scalar, rows, columns) = shape.Value;

            if (columns == 0)
            {
                return HlslType.Vector(Kind(scalar), rows);
            }

            return HlslType.Matrix(Kind(scalar), rows, columns);
        }

        return Template(syntax);
    }

    private HlslType Template(TypeSyntax syntax)
    {
        if (syntax.Name == "vector" && syntax.Argument is not null)
        {
            return HlslType.Vector(Base(syntax.Argument).Kind, (int)syntax.Sizes[0]);
        }

        if (syntax.Name == "void")
        {
            return HlslType.Void;
        }

        HlslType? element = null;

        if (syntax.Argument is not null)
        {
            element = Base(syntax.Argument);
        }

        return HlslType.Resource(syntax.Name, element);
    }

    public static ScalarKind Kind(string scalar)
    {
        return scalar switch
        {
            "bool" => ScalarKind.Bool,
            "int" or "min16int" or "int32_t" or "int16_t" => ScalarKind.Int,
            "uint" or "dword" or "min16uint" or "uint32_t" or "uint16_t" => ScalarKind.Uint,
            "double" or "float64_t" => ScalarKind.Double,
            "int64_t" => ScalarKind.Int64,
            "uint64_t" => ScalarKind.Uint64,
            _ => ScalarKind.Float,
        };
    }
}
