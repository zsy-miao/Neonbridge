// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.Ffx;

/// <summary>
/// The FidelityFX API facts the shim depends on (docs/middleware.md): return codes and descriptor type ids of the
/// public ffx_api.h, ffx_api_dx12.h and ffx_upscale.h, and the descriptor offsets the game fills, read from its calls.
/// Every descriptor starts with a header: the type (UINT64) at 0, the next descriptor of the chain at 8.
/// </summary>
internal static class FfxLayout
{
    /// <summary>The console loader the game imports ffxCreateContext, ffxDestroyContext, ffxDispatch and ffxQuery from.</summary>
    public const string Module = "amd_fidelityfx_loader_xbox.dll";

    /// <summary>FFX_API_RETURN_OK.</summary>
    public const uint ReturnOk = 0;

    /// <summary>FFX_API_RETURN_ERROR_UNKNOWN_DESCTYPE.</summary>
    public const uint ReturnUnknownDescType = 2;

    /// <summary>FFX_API_RETURN_NO_PROVIDER.</summary>
    public const uint ReturnNoProvider = 4;

    /// <summary>FFX_API_RETURN_ERROR_PARAMETER: what the console loader returns for a null context or descriptor.</summary>
    public const uint ReturnParameter = 6;

    public const int NextOffset = 8;

    /// <summary>The first field after the header.</summary>
    public const int PayloadOffset = 0x10;

    /// <summary>The console's D3D12 backend descriptor: { header; device at +0x10 } (read from the console upscaler).</summary>
    public const ulong XboxBackendDescType = 0x01000002;

    /// <summary>FFX_API_CREATE_CONTEXT_DESC_TYPE_BACKEND_DX12 (ffx_api_dx12.h): { header; ID3D12Device* device }.</summary>
    public const ulong PcBackendDescType = 0x00000002;

    /// <summary>FFX_API_DISPATCH_DESC_TYPE_UPSCALE.</summary>
    public const ulong DispatchUpscaleType = 0x00010001;

    /// <summary>FFX_API_QUERY_DESC_TYPE_UPSCALE_GETJITTERPHASECOUNT: { header; renderWidth; displayWidth; int32* out }.</summary>
    public const ulong QueryJitterPhaseCountType = 0x00010004;

    /// <summary>FFX_API_QUERY_DESC_TYPE_UPSCALE_GETJITTEROFFSET: { header; index; phaseCount; float* x; float* y }.</summary>
    public const ulong QueryJitterOffsetType = 0x00010005;

    /// <summary>FFX_API_QUERY_DESC_TYPE_UPSCALE_GPU_MEMORY_USAGE: { header; FfxApiEffectMemoryUsage* out } (two UINT64).</summary>
    public const ulong QueryGpuMemoryUsageType = 0x00010008;

    public const int JitterDisplayWidthOffset = 0x14;
    public const int JitterPhaseCountOutOffset = 0x18;
    public const int JitterPhaseCountOffset = 0x14;
    public const int JitterOffsetXOutOffset = 0x18;
    public const int JitterOffsetYOutOffset = 0x20;

    /// <summary>Bytes of FfxApiEffectMemoryUsage: totalUsageInBytes and aliasableUsageInBytes.</summary>
    public const int MemoryUsageSize = 16;

    /// <summary>ffxDispatchDescUpscale: the command list follows the header.</summary>
    public const int DispatchCommandListOffset = 0x10;

    /// <summary>ffxDispatchDescUpscale: color, depth, motion vectors, exposure, reactive, transparency and output.</summary>
    public const int DispatchFirstResourceOffset = 0x18;

    public const int DispatchResourceCount = 7;

    /// <summary>Bytes of FfxApiResource: the resource pointer at 0, its description and state after it.</summary>
    public const int ResourceStride = 0x30;

    /// <summary>A guard against a descriptor chain that loops.</summary>
    public const int MaxChainLength = 64;
}
