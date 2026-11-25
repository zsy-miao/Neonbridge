// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Dxil.Lowering;

/// <summary>
/// An HLSL type. Values of every type but resources are lowered as their flattened scalar components: a vector's in
/// order, a matrix's row by row, an array's elements, a struct's fields.
/// </summary>
public sealed class HlslType
{
    private const int MinColumns = 2;
    private const int MaxColumns = 4;

    // Every vector type made once up front, so lookups need no lock while programs compile in parallel.
    private static readonly HlslType[,] Vectors = MakeVectors();

    private HlslType(TypeShape shape)
    {
        Shape = shape;
    }

    public static HlslType Void { get; } = new(TypeShape.Void);

    public static HlslType Bool { get; } = Scalar(ScalarKind.Bool);

    public static HlslType Int { get; } = Scalar(ScalarKind.Int);

    public static HlslType Uint { get; } = Scalar(ScalarKind.Uint);

    public static HlslType Float { get; } = Scalar(ScalarKind.Float);

    public static HlslType Double { get; } = Scalar(ScalarKind.Double);

    public static HlslType Int64 { get; } = Scalar(ScalarKind.Int64);

    public static HlslType Uint64 { get; } = Scalar(ScalarKind.Uint64);

    public static HlslType LiteralInt { get; } = Scalar(ScalarKind.LiteralInt);

    public static HlslType LiteralFloat { get; } = Scalar(ScalarKind.LiteralFloat);

    public TypeShape Shape { get; }

    public ScalarKind Kind { get; private init; }

    /// <summary>Vector size, or matrix columns.</summary>
    public int Columns { get; private init; } = 1;

    public int Rows { get; private init; } = 1;

    /// <summary>An array's element type, or a resource's template type.</summary>
    public HlslType? Element { get; private init; }

    public int Length { get; private init; }

    public string Name { get; private init; } = string.Empty;

    public IReadOnlyList<(string Name, HlslType Type)> Fields { get; private init; } = [];

    /// <summary>A resource's HLSL type name (Texture2D, RWByteAddressBuffer, SamplerState...).</summary>
    public string ResourceName => Name;

    /// <summary>The number of scalar components.</summary>
    public int Count { get; private init; } = 1;

    public bool IsNumeric => Shape is TypeShape.Scalar or TypeShape.Vector or TypeShape.Matrix;

    public bool IsLiteral => Kind is ScalarKind.LiteralInt or ScalarKind.LiteralFloat;

    public static HlslType Scalar(ScalarKind kind)
    {
        return new HlslType(TypeShape.Scalar) { Kind = kind };
    }

    /// <summary>A vector (size 1 is the scalar).</summary>
    public static HlslType Vector(ScalarKind kind, int size)
    {
        if (size == 1)
        {
            return ScalarOf(kind);
        }

        if (size > MaxColumns)
        {
            return new HlslType(TypeShape.Vector) { Kind = kind, Columns = size, Count = size };
        }

        return Vectors[(int)kind, size];
    }

    private static HlslType[,] MakeVectors()
    {
        var kinds = Enum.GetValues<ScalarKind>();
        var vectors = new HlslType[kinds.Length, MaxColumns + 1];

        foreach (var kind in kinds)
        {
            for (var size = MinColumns; size <= MaxColumns; size++)
            {
                vectors[(int)kind, size] = new HlslType(TypeShape.Vector) { Kind = kind, Columns = size, Count = size };
            }
        }

        return vectors;
    }

    public static HlslType Matrix(ScalarKind kind, int rows, int columns)
    {
        return new HlslType(TypeShape.Matrix) { Kind = kind, Rows = rows, Columns = columns, Count = rows * columns };
    }

    public static HlslType Array(HlslType element, int length)
    {
        return new HlslType(TypeShape.Array) { Element = element, Length = length, Count = element.Count * length };
    }

    public static HlslType Struct(string name, IReadOnlyList<(string Name, HlslType Type)> fields)
    {
        return new HlslType(TypeShape.Struct) { Name = name, Fields = fields, Count = fields.Sum(field => field.Type.Count) };
    }

    public static HlslType Resource(string name, HlslType? element)
    {
        return new HlslType(TypeShape.Resource) { Name = name, Element = element, Count = 1 };
    }

    public static HlslType ScalarOf(ScalarKind kind)
    {
        return kind switch
        {
            ScalarKind.Bool => Bool,
            ScalarKind.Int => Int,
            ScalarKind.Uint => Uint,
            ScalarKind.Float => Float,
            ScalarKind.Double => Double,
            ScalarKind.Int64 => Int64,
            ScalarKind.Uint64 => Uint64,
            ScalarKind.LiteralInt => LiteralInt,
            _ => LiteralFloat,
        };
    }

    /// <summary>The same shape with another scalar kind (for numeric types).</summary>
    public HlslType WithKind(ScalarKind kind)
    {
        return Shape switch
        {
            TypeShape.Vector => Vector(kind, Columns),
            TypeShape.Matrix => Matrix(kind, Rows, Columns),
            _ => ScalarOf(kind),
        };
    }

    /// <summary>The scalar kind of flattened component <paramref name="index"/>.</summary>
    public ScalarKind ComponentKind(int index)
    {
        if (Shape == TypeShape.Array)
        {
            return Element!.ComponentKind(index % Element.Count);
        }

        if (Shape != TypeShape.Struct)
        {
            return Kind;
        }

        foreach (var (_, type) in Fields)
        {
            if (index < type.Count)
            {
                return type.ComponentKind(index);
            }

            index -= type.Count;
        }

        throw new ArgumentOutOfRangeException(nameof(index));
    }

    public bool SameAs(HlslType other)
    {
        if (ReferenceEquals(this, other))
        {
            return true;
        }

        if (Shape != other.Shape || Count != other.Count)
        {
            return false;
        }

        return Shape switch
        {
            TypeShape.Struct => Name == other.Name,
            TypeShape.Array => Length == other.Length && Element!.SameAs(other.Element!),
            TypeShape.Resource => Name == other.Name,
            _ => Kind == other.Kind && Columns == other.Columns && Rows == other.Rows,
        };
    }

    public override string ToString()
    {
        return Shape switch
        {
            TypeShape.Vector => $"{Kind}{Columns}",
            TypeShape.Matrix => $"{Kind}{Rows}x{Columns}",
            TypeShape.Array => $"{Element}[{Length}]",
            TypeShape.Struct or TypeShape.Resource => Name,
            _ => Kind.ToString(),
        };
    }
}
