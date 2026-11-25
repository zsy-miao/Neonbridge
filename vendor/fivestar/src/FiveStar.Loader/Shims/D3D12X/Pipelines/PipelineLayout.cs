// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.D3D12X.Pipelines;

/// <summary>
/// The pipeline-creation facts of the D3D12.X device, read from the game's call sites (docs/d3d12x-pipelines.md), and
/// the desktop d3d12.h struct sizes the descriptions carry.
/// </summary>
internal static class PipelineLayout
{
    /// <summary>Device slot: derive a pipeline from a base pipeline and state overrides.</summary>
    public const int DeriveSlot = 55;

    /// <summary>Device slot: graphics pipeline from a pipeline-cache record.</summary>
    public const int PrecompiledGraphicsSlot = 57;

    /// <summary>Device slot: compute pipeline from a D3D12_COMPUTE_PIPELINE_STATE_DESC and two zero arguments.</summary>
    public const int ComputeSlot = 62;

    /// <summary>Device slot: compute pipeline from a pipeline-cache record.</summary>
    public const int PrecompiledComputeSlot = 67;

    /// <summary>Device slot: pipeline from a D3D12_PIPELINE_STATE_STREAM_DESC (desktop ID3D12Device2 slot 47).</summary>
    public const int StreamSlot = 86;

    /// <summary>A pointer, and the alignment of every stream subobject.</summary>
    public const int PointerSize = sizeof(long);

    /// <summary>A {pointer, u64 size} pair (D3D12_SHADER_BYTECODE and the cache ranges).</summary>
    public const int RangeSize = 2 * sizeof(long);

    // D3D12_COMPUTE_PIPELINE_STATE_DESC: pRootSignature @0, CS @8.
    public const int ComputeShaderField = 0x08;

    // Pipeline-cache graphics description (0x108 bytes, built by 0x14248f680): root signature @0, state range @8,
    // input-layout range @18, seven slots @28 of {code range, shader-metadata range}.
    public const int CachedStateField = 0x08;
    public const int CachedInputLayoutField = 0x18;
    public const int CachedSlotsField = 0x28;
    public const int CachedSlotCount = 7;
    public const int CachedSlotSize = 2 * RangeSize;

    // Pipeline-cache compute description (0x48 bytes): root signature @0, state range @8, second metadata range @18,
    // shader slot @28 of {code range, shader-metadata range}.
    public const int CachedComputeShaderField = 0x28;

    // Override entries of the derive slot: u32 kind, then the desc (the largest is the blend desc).
    public const int OverrideStride = 0x14C;
    public const int OverridePayloadField = sizeof(int);

    // Desktop struct sizes (d3d12.h).
    public const int BlendDescSize = 0x148;
    public const int RasterizerDescSize = 0x2C;
    public const int DepthStencilDescSize = 0x34;
    public const int DepthStencil1DescSize = 0x38;
    public const int InputElementSize = 0x20;
    public const int RenderTargetSlots = 8;

    /// <summary>D3D12_PRIMITIVE_TOPOLOGY_TYPE_TRIANGLE.</summary>
    public const int TriangleTopology = 3;
}
