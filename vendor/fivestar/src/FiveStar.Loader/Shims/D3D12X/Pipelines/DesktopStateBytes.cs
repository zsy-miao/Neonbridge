// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Buffers.Binary;

namespace FiveStar.Loader.Shims.D3D12X.Pipelines;

/// <summary>
/// The desktop blend, rasterizer and depth-stencil descs as bytes in their d3d12.h layout: the stream defaults and the
/// writers the pipeline-cache path fills them with.
/// </summary>
internal static class DesktopStateBytes
{
    // D3D12_BLEND_DESC: AlphaToCoverageEnable @0, IndependentBlendEnable @4, RenderTarget[8] @8 of 0x28 bytes.
    public const int IndependentBlendField = 0x04;
    public const int TargetsField = 0x08;
    public const int TargetSize = 0x28;

    // D3D12_RENDER_TARGET_BLEND_DESC.
    public const int TargetEnableField = 0x00;
    public const int SrcBlendField = 0x08;
    public const int DestBlendField = 0x0C;
    public const int BlendOpField = 0x10;
    public const int SrcBlendAlphaField = 0x14;
    public const int DestBlendAlphaField = 0x18;
    public const int BlendOpAlphaField = 0x1C;
    public const int LogicOpField = 0x20;
    public const int WriteMaskField = 0x24;

    // D3D12_RASTERIZER_DESC.
    public const int FillModeField = 0x00;
    public const int CullModeField = 0x04;
    public const int FrontCounterClockwiseField = 0x08;
    public const int DepthBiasField = 0x0C;
    public const int DepthBiasClampField = 0x10;
    public const int SlopeScaledDepthBiasField = 0x14;
    public const int DepthClipField = 0x18;
    public const int ForcedSampleCountField = 0x24;

    // D3D12_DEPTH_STENCIL_DESC1 (the first 0x34 bytes are D3D12_DEPTH_STENCIL_DESC).
    public const int DepthEnableField = 0x00;
    public const int DepthWriteMaskField = 0x04;
    public const int DepthFuncField = 0x08;
    public const int StencilEnableField = 0x0C;
    public const int StencilReadMaskField = 0x10;
    public const int StencilWriteMaskField = 0x11;
    public const int FrontFaceField = 0x14;
    public const int BackFaceField = 0x24;
    public const int FaceFieldSize = sizeof(int);

    // Enum values (d3d12.h).
    public const int BlendZero = 1;
    public const int BlendOne = 2;
    public const int BlendOpAdd = 1;
    public const int LogicOpNoop = 4;
    public const byte WriteAll = 0x0F;
    public const int FillSolid = 3;
    public const int CullNone = 1;
    public const int CullBack = 3;
    public const int DepthWriteAll = 1;
    public const int ComparisonLess = 2;
    public const int ComparisonAlways = 8;
    public const int StencilKeep = 1;
    public const byte StencilMaskAll = 0xFF;

    /// <summary>D3D12_DEFAULT blend: every target writes all channels, blending off.</summary>
    public static byte[] DefaultBlend()
    {
        var blend = new byte[PipelineLayout.BlendDescSize];

        for (var target = 0; target < PipelineLayout.RenderTargetSlots; target++)
        {
            WriteTarget(blend, target, false, [BlendOne, BlendZero, BlendOpAdd, BlendOne, BlendZero, BlendOpAdd], WriteAll);
        }

        return blend;
    }

    /// <summary>
    /// Writes one D3D12_RENDER_TARGET_BLEND_DESC; <paramref name="factors"/> is SrcBlend, DestBlend, BlendOp,
    /// SrcBlendAlpha, DestBlendAlpha, BlendOpAlpha.
    /// </summary>
    public static void WriteTarget(Span<byte> blend, int target, bool enable, ReadOnlySpan<int> factors, byte writeMask)
    {
        var desc = blend.Slice(TargetsField + (target * TargetSize), TargetSize);
        int[] fields = [SrcBlendField, DestBlendField, BlendOpField, SrcBlendAlphaField, DestBlendAlphaField, BlendOpAlphaField];
        Write(desc, TargetEnableField, enable);

        for (var index = 0; index < fields.Length; index++)
        {
            BinaryPrimitives.WriteInt32LittleEndian(desc[fields[index]..], factors[index]);
        }

        BinaryPrimitives.WriteInt32LittleEndian(desc[LogicOpField..], LogicOpNoop);
        desc[WriteMaskField] = writeMask;
    }

    /// <summary>D3D12_DEFAULT rasterizer: solid, back faces culled, clockwise front, depth clip on.</summary>
    public static byte[] DefaultRasterizer()
    {
        var rasterizer = new byte[PipelineLayout.RasterizerDescSize];
        BinaryPrimitives.WriteInt32LittleEndian(rasterizer.AsSpan(FillModeField), FillSolid);
        BinaryPrimitives.WriteInt32LittleEndian(rasterizer.AsSpan(CullModeField), CullBack);
        Write(rasterizer, DepthClipField, true);

        return rasterizer;
    }

    /// <summary>D3D12_DEFAULT depth-stencil: depth test LESS with writes, stencil off, depth bounds off.</summary>
    public static byte[] DefaultDepthStencil()
    {
        var depth = new byte[PipelineLayout.DepthStencil1DescSize];
        Write(depth, DepthEnableField, true);
        BinaryPrimitives.WriteInt32LittleEndian(depth.AsSpan(DepthWriteMaskField), DepthWriteAll);
        BinaryPrimitives.WriteInt32LittleEndian(depth.AsSpan(DepthFuncField), ComparisonLess);
        depth[StencilReadMaskField] = StencilMaskAll;
        depth[StencilWriteMaskField] = StencilMaskAll;

        WriteFace(depth, FrontFaceField, [StencilKeep, StencilKeep, StencilKeep, ComparisonAlways]);
        WriteFace(depth, BackFaceField, [StencilKeep, StencilKeep, StencilKeep, ComparisonAlways]);

        return depth;
    }

    /// <summary>A D3D12_DEPTH_STENCIL_DESC widened to D3D12_DEPTH_STENCIL_DESC1 with depth bounds off.</summary>
    public static byte[] WidenDepthStencil(ReadOnlySpan<byte> depthStencil)
    {
        var depth = new byte[PipelineLayout.DepthStencil1DescSize];
        depthStencil[..PipelineLayout.DepthStencilDescSize].CopyTo(depth);

        return depth;
    }

    /// <summary>Writes one D3D12_DEPTH_STENCILOP_DESC: fail, depth-fail and pass ops, then the function.</summary>
    public static void WriteFace(Span<byte> depth, int field, ReadOnlySpan<int> face)
    {
        for (var index = 0; index < face.Length; index++)
        {
            BinaryPrimitives.WriteInt32LittleEndian(depth[(field + (index * FaceFieldSize))..], face[index]);
        }
    }

    /// <summary>Writes a BOOL.</summary>
    public static void Write(Span<byte> desc, int field, bool value)
    {
        BinaryPrimitives.WriteInt32LittleEndian(desc[field..], Convert.ToInt32(value));
    }
}
