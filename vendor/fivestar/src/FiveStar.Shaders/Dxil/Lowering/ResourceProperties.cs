// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Dxil.Ir;

namespace FiveStar.Shaders.Dxil.Lowering;

/// <summary>
/// The two dwords of <c>dx.types.ResourceProperties</c> for an HLSL resource type (DxilResourceProperties): dword 0 is
/// the resource kind with the UAV (bit 12) and comparison-sampler (bit 15) flags; dword 1 a structured buffer's
/// stride or a typed resource's component type and count.
/// </summary>
public static class ResourceProperties
{
    public const uint UavBit = 0x1000;
    public const uint ComparisonBit = 0x8000;

    private const int CountShift = 8;

    private static readonly Dictionary<string, ResourceKind> Kinds = new(StringComparer.Ordinal)
    {
        ["ByteAddressBuffer"] = ResourceKind.RawBuffer,
        ["StructuredBuffer"] = ResourceKind.StructuredBuffer,
        ["Buffer"] = ResourceKind.TypedBuffer,
        ["Texture1D"] = ResourceKind.Texture1D,
        ["Texture1DArray"] = ResourceKind.Texture1DArray,
        ["Texture2D"] = ResourceKind.Texture2D,
        ["Texture2DArray"] = ResourceKind.Texture2DArray,
        ["Texture2DMS"] = ResourceKind.Texture2DMS,
        ["Texture2DMSArray"] = ResourceKind.Texture2DMSArray,
        ["Texture3D"] = ResourceKind.Texture3D,
        ["TextureCube"] = ResourceKind.TextureCube,
        ["TextureCubeArray"] = ResourceKind.TextureCubeArray,
        ["SamplerState"] = ResourceKind.Sampler,
        ["SamplerComparisonState"] = ResourceKind.Sampler,
        ["RaytracingAccelerationStructure"] = ResourceKind.AccelerationStructure,
    };

    /// <summary>The DXIL resource kinds (DXIL::ResourceKind).</summary>
    public enum ResourceKind
    {
        Texture1D = 1,
        Texture2D = 2,
        Texture2DMS = 3,
        Texture3D = 4,
        TextureCube = 5,
        Texture1DArray = 6,
        Texture2DArray = 7,
        Texture2DMSArray = 8,
        TextureCubeArray = 9,
        TypedBuffer = 10,
        RawBuffer = 11,
        StructuredBuffer = 12,
        Sampler = 14,
        AccelerationStructure = 16,
    }

    /// <summary>DXIL component types (DXIL::ComponentType) of typed resources.</summary>
    public enum ComponentType
    {
        I32 = 4,
        U32 = 5,
        I64 = 6,
        U64 = 7,
        F32 = 9,
    }

    /// <summary>Whether the type is writable: RW and rasterizer-ordered resources.</summary>
    public static bool IsUav(string name)
    {
        return name.StartsWith("RW", StringComparison.Ordinal) || name.StartsWith("RasterizerOrdered", StringComparison.Ordinal);
    }

    /// <summary>The base kind name without the RW or RasterizerOrdered prefix.</summary>
    public static string BaseName(string name)
    {
        if (name.StartsWith("RasterizerOrdered", StringComparison.Ordinal))
        {
            return name["RasterizerOrdered".Length..];
        }

        if (name.StartsWith("RW", StringComparison.Ordinal))
        {
            return name[sizeof(char)..];
        }

        return name;
    }

    public static ResourceKind KindOf(HlslType type)
    {
        if (!Kinds.TryGetValue(BaseName(type.Name), out var kind))
        {
            throw new DxilBackendException($"resource type {type.Name} is not supported");
        }

        return kind;
    }

    public static (uint Kind, uint Detail) Of(HlslType type, DxilModule module)
    {
        var kind = KindOf(type);
        var word = (uint)kind;

        if (IsUav(type.Name))
        {
            word |= UavBit;
        }

        if (type.Name == "SamplerComparisonState")
        {
            word |= ComparisonBit;
        }

        return (word, Detail(type, kind));
    }

    private static uint Detail(HlslType type, ResourceKind kind)
    {
        if (kind is ResourceKind.RawBuffer or ResourceKind.Sampler or ResourceKind.AccelerationStructure)
        {
            return 0;
        }

        var element = type.Element ?? HlslType.Vector(ScalarKind.Float, sizeof(uint));

        if (kind == ResourceKind.StructuredBuffer)
        {
            return (uint)(element.Count * sizeof(uint));
        }

        var component = element.Kind switch
        {
            ScalarKind.Int => ComponentType.I32,
            ScalarKind.Uint or ScalarKind.Bool => ComponentType.U32,
            ScalarKind.Int64 => ComponentType.I64,
            ScalarKind.Uint64 => ComponentType.U64,
            _ => ComponentType.F32,
        };

        return (uint)component | ((uint)element.Count << CountShift);
    }
}
