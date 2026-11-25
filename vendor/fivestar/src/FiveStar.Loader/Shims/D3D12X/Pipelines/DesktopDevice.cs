// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.D3D12X.Pipelines;

/// <summary>The desktop ID3D12Device2 methods pipeline creation calls, through the vtable (d3d12.h slot order).</summary>
internal static unsafe class DesktopDevice
{
    /// <summary>IID_ID3D12Device2.</summary>
    public static readonly Guid Device2Iid = new("30baa41e-b15b-475c-a0bb-1af5c5b64328");

    /// <summary>IID_ID3D12RootSignature.</summary>
    public static readonly Guid RootSignatureIid = new("c54a6b66-72df-4ee8-8be5-a946a1429214");

    /// <summary>IID_ID3D12PipelineState.</summary>
    public static readonly Guid PipelineStateIid = new("765a30f3-f624-4c6f-a828-ace948622445");

    /// <summary>IID_IUnknown (unknwn.h).</summary>
    public static readonly Guid UnknownIid = new("00000000-0000-0000-c000-000000000046");

    private const int QueryInterfaceSlot = 0;
    private const int AddRefSlot = 1;
    private const int ReleaseSlot = 2;
    private const int CheckFeatureSupportSlot = 13;
    /// <summary>IID_ID3D12PipelineLibrary1.</summary>
    public static readonly Guid PipelineLibrary1Iid = new("80eabf42-2568-4e5e-bd82-c37f86961dc3");

    private const int CreateRootSignatureSlot = 16;
    private const int CreatePipelineLibrarySlot = 44;
    private const int CreatePipelineStateSlot = 47;

    public static int QueryInterface(nint unknown, Guid* iid, nint* result)
    {
        return ((delegate* unmanaged<nint, Guid*, nint*, int>)Method(unknown, QueryInterfaceSlot))(unknown, iid, result);
    }

    public static uint AddRef(nint unknown)
    {
        return ((delegate* unmanaged<nint, uint>)Method(unknown, AddRefSlot))(unknown);
    }

    public static uint Release(nint unknown)
    {
        return ((delegate* unmanaged<nint, uint>)Method(unknown, ReleaseSlot))(unknown);
    }

    /// <summary>ID3D12Device::CheckFeatureSupport(D3D12_FEATURE, void* data, UINT size).</summary>
    public static int CheckFeatureSupport(nint device, int feature, void* data, uint size)
    {
        return ((delegate* unmanaged<nint, int, void*, uint, int>)Method(device, CheckFeatureSupportSlot))(device, feature, data, size);
    }

    /// <summary>ID3D12Device::CreateRootSignature(UINT nodeMask, const void* blob, SIZE_T size, REFIID, void**).</summary>
    public static int CreateRootSignature(nint device, void* blob, nuint size, nint* result)
    {
        var target = (delegate* unmanaged<nint, uint, void*, nuint, Guid*, nint*, int>)Method(device, CreateRootSignatureSlot);
        var iid = RootSignatureIid;

        return target(device, 0, blob, size, &iid, result);
    }

    /// <summary>ID3D12Device2::CreatePipelineState(const D3D12_PIPELINE_STATE_STREAM_DESC*, REFIID, void**).</summary>
    public static int CreatePipelineState(nint device2, void* streamDesc, Guid* iid, nint* result)
    {
        var target = (delegate* unmanaged<nint, void*, Guid*, nint*, int>)Method(device2, CreatePipelineStateSlot);

        return target(device2, streamDesc, iid, result);
    }

    /// <summary>
    /// ID3D12Device1::CreatePipelineLibrary(const void* blob, SIZE_T size, REFIID, void**) for ID3D12PipelineLibrary1;
    /// the blob (null and 0 for an empty library) must stay valid while the library lives.
    /// </summary>
    public static int CreatePipelineLibrary(nint device1, void* blob, nuint size, nint* result)
    {
        var target = (delegate* unmanaged<nint, void*, nuint, Guid*, nint*, int>)Method(device1, CreatePipelineLibrarySlot);
        var iid = PipelineLibrary1Iid;

        return target(device1, blob, size, &iid, result);
    }

    private static nint Method(nint unknown, int slot)
    {
        return (*(nint**)unknown)[slot];
    }
}
