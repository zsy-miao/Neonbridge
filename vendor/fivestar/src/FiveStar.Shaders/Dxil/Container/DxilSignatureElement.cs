// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Dxil.Container;

/// <summary>
/// One element of a DXIL signature, as the entry metadata, ISG1/OSG1/PSG1 and PSV0 describe it. The packer sets the
/// start row and column (-1: no register); the lowering records the components the program reads or writes.
/// </summary>
public sealed class DxilSignatureElement
{
    /// <summary>DXIL component types (DxilConstants.h ComponentType).</summary>
    public const byte Bool = 1;
    public const byte Int32 = 4;
    public const byte Uint32 = 5;
    public const byte Float32 = 9;

    /// <summary>D3D_REGISTER_COMPONENT_TYPE: the component types of signature parts and PSV0.</summary>
    private const byte RegisterUint = 1;
    private const byte RegisterInt = 2;
    private const byte RegisterFloat = 3;

    private static readonly Dictionary<SemanticKind, SystemValueName> Names = new()
    {
        [SemanticKind.Position] = SystemValueName.Position,
        [SemanticKind.ClipDistance] = SystemValueName.ClipDistance,
        [SemanticKind.CullDistance] = SystemValueName.CullDistance,
        [SemanticKind.RenderTargetArrayIndex] = SystemValueName.RenderTargetArrayIndex,
        [SemanticKind.ViewportArrayIndex] = SystemValueName.ViewportArrayIndex,
        [SemanticKind.VertexId] = SystemValueName.VertexId,
        [SemanticKind.PrimitiveId] = SystemValueName.PrimitiveId,
        [SemanticKind.InstanceId] = SystemValueName.InstanceId,
        [SemanticKind.IsFrontFace] = SystemValueName.IsFrontFace,
        [SemanticKind.SampleIndex] = SystemValueName.SampleIndex,
        [SemanticKind.Barycentrics] = SystemValueName.Barycentrics,
        [SemanticKind.ShadingRate] = SystemValueName.ShadingRate,
        [SemanticKind.CullPrimitive] = SystemValueName.CullPrimitive,
        [SemanticKind.Target] = SystemValueName.Target,
        [SemanticKind.Depth] = SystemValueName.Depth,
        [SemanticKind.Coverage] = SystemValueName.Coverage,
        [SemanticKind.DepthGreaterEqual] = SystemValueName.DepthGreaterEqual,
        [SemanticKind.DepthLessEqual] = SystemValueName.DepthLessEqual,
        [SemanticKind.StencilRef] = SystemValueName.StencilRef,
        [SemanticKind.InnerCoverage] = SystemValueName.InnerCoverage,
    };

    public required int Id { get; init; }

    /// <summary>The semantic without its index (system values in their canonical spelling).</summary>
    public required string Name { get; init; }

    public required int Index { get; init; }

    public required SemanticKind Kind { get; init; }

    public required SemanticInterpretation Interpretation { get; init; }

    public required byte ComponentType { get; init; }

    /// <summary>DXIL interpolation mode (0 where the signature point has none).</summary>
    public required byte Interpolation { get; init; }

    public required int Columns { get; init; }

    public int Rows { get; init; } = 1;

    public int StartRow { get; set; } = -1;

    public int StartColumn { get; set; } = -1;

    /// <summary>Components read (inputs) or written (outputs), from bit 0 = the element's first column.</summary>
    public byte Usage { get; set; }

    public bool IsAllocated => StartRow >= 0;

    /// <summary>The element's columns as a register mask (from column 0 when it has no register).</summary>
    public byte Mask => (byte)(((1 << Columns) - 1) << Math.Max(StartColumn, 0));

    /// <summary>The component type of signature parts and PSV0.</summary>
    public byte RegisterComponentType
    {
        get
        {
            if (ComponentType == Float32)
            {
                return RegisterFloat;
            }

            if (ComponentType == Int32)
            {
                return RegisterInt;
            }

            return RegisterUint;
        }
    }

    public SystemValueName SystemValue => Names.GetValueOrDefault(Kind);

    /// <summary>Whether the element is listed in the signature (everything but values read through their own operation).</summary>
    public bool InSignature => Interpretation != SemanticInterpretation.NotInSignature;

    public override string ToString()
    {
        return $"{Name}{Index} r{StartRow} c{StartColumn} x{Columns}";
    }
}
