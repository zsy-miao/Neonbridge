// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Dxil.Hlsl;

/// <summary>Which names are types: the built-in scalars, their vectors and matrices, resource types, and declared structs.</summary>
public sealed class TypeNames
{
    private const int MaxColumns = 4;
    private const int MatrixSuffix = 3;
    private const int MatrixSeparator = 2;

    private static readonly HashSet<string> Scalars = new(StringComparer.Ordinal)
    {
        "bool", "int", "uint", "dword", "float", "half", "double", "min16float", "min16int", "min16uint", "min10float",
        "int64_t", "uint64_t", "int16_t", "uint16_t", "float16_t", "float32_t", "float64_t", "int32_t", "uint32_t",
    };

    private static readonly HashSet<string> Others = new(StringComparer.Ordinal)
    {
        "void", "vector", "matrix", "ByteAddressBuffer", "RWByteAddressBuffer", "StructuredBuffer", "RWStructuredBuffer",
        "Buffer", "RWBuffer", "Texture1D", "Texture1DArray", "Texture2D", "Texture2DArray", "Texture2DMS", "Texture2DMSArray",
        "Texture3D", "TextureCube", "TextureCubeArray", "RWTexture1D", "RWTexture1DArray", "RWTexture2D", "RWTexture2DArray",
        "RWTexture3D", "RasterizerOrderedTexture2D", "RasterizerOrderedTexture2DArray", "RasterizerOrderedTexture3D",
        "RasterizerOrderedBuffer", "RasterizerOrderedByteAddressBuffer", "SamplerState", "SamplerComparisonState",
        "RaytracingAccelerationStructure", "ConstantBuffer",
    };

    private readonly HashSet<string> _structs = new(StringComparer.Ordinal);

    public void AddStruct(string name)
    {
        _structs.Add(name);
    }

    public bool IsType(string name)
    {
        return Scalars.Contains(name) || Others.Contains(name) || _structs.Contains(name) || IsVectorOrMatrix(name);
    }

    public bool IsStruct(string name)
    {
        return _structs.Contains(name);
    }

    /// <summary>Splits <c>uint4</c> into (uint, 4, 0) and <c>float3x4</c> into (float, 3, 4); null for other names.</summary>
    public static (string Scalar, int Rows, int Columns)? Shape(string name)
    {
        if (name.Length < MatrixSuffix || !char.IsDigit(name[^1]))
        {
            return null;
        }

        var columns = name[^1] - '0';

        if (name.Length > MatrixSuffix && name[^MatrixSeparator] == 'x' && char.IsDigit(name[^MatrixSuffix]))
        {
            var scalar = name[..^MatrixSuffix];
            var rows = name[^MatrixSuffix] - '0';

            if (Scalars.Contains(scalar) && Valid(rows) && Valid(columns))
            {
                return (scalar, rows, columns);
            }

            return null;
        }

        var vectorScalar = name[..^1];

        if (Scalars.Contains(vectorScalar) && Valid(columns))
        {
            return (vectorScalar, columns, 0);
        }

        return null;
    }

    public static bool IsScalar(string name)
    {
        return Scalars.Contains(name);
    }

    private static bool IsVectorOrMatrix(string name)
    {
        return Shape(name) is not null;
    }

    private static bool Valid(int size)
    {
        return size is >= 1 and <= MaxColumns;
    }
}
