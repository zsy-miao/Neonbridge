// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.D3D12X.Pipelines;

/// <summary>
/// The ID3D12PipelineLibrary1 methods the pipeline cache calls, through the vtable (d3d12.h slot order: IUnknown,
/// ID3D12Object, ID3D12DeviceChild, then StorePipeline, LoadGraphicsPipeline, LoadComputePipeline, GetSerializedSize,
/// Serialize and LoadPipeline).
/// </summary>
internal static unsafe class DesktopPipelineLibrary
{
    /// <summary>D3D12_ERROR_ADAPTER_NOT_FOUND: the blob was made on another adapter.</summary>
    public const int AdapterNotFound = unchecked((int)0x887E0001);

    /// <summary>D3D12_ERROR_DRIVER_VERSION_MISMATCH: the blob was made by another driver version.</summary>
    public const int DriverVersionMismatch = unchecked((int)0x887E0002);

    private const int StorePipelineSlot = 8;
    private const int GetSerializedSizeSlot = 11;
    private const int SerializeSlot = 12;
    private const int LoadPipelineSlot = 13;

    /// <summary>StorePipeline(LPCWSTR name, ID3D12PipelineState*); E_INVALIDARG when the name is taken.</summary>
    public static int Store(nint library, string name, nint pipeline)
    {
        fixed (char* text = name)
        {
            return ((delegate* unmanaged<nint, char*, nint, int>)ComObject.Slot(library, StorePipelineSlot))(library, text, pipeline);
        }
    }

    /// <summary>
    /// LoadPipeline(LPCWSTR name, const D3D12_PIPELINE_STATE_STREAM_DESC*, REFIID, void**); E_INVALIDARG when the name
    /// is missing or was stored with another description.
    /// </summary>
    public static int Load(nint library, string name, void* streamDesc, Guid* iid, nint* result)
    {
        fixed (char* text = name)
        {
            var target = (delegate* unmanaged<nint, char*, void*, Guid*, nint*, int>)ComObject.Slot(library, LoadPipelineSlot);

            return target(library, text, streamDesc, iid, result);
        }
    }

    public static nuint SerializedSize(nint library)
    {
        return ((delegate* unmanaged<nint, nuint>)ComObject.Slot(library, GetSerializedSizeSlot))(library);
    }

    public static int Serialize(nint library, void* data, nuint size)
    {
        return ((delegate* unmanaged<nint, void*, nuint, int>)ComObject.Slot(library, SerializeSlot))(library, data, size);
    }
}
