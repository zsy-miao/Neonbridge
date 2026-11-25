// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Text;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.D3D12X.Binding;

/// <summary>
/// A small compute program that writes one value over a texture UAV of the shader-visible view heap, by heap index: a
/// float4 for a float, unorm or snorm view, a uint for a uint view (an R9G9B9E5 colour packed, as the game's
/// R32_UINT views of such textures hold it). 2D, 2D-array and 3D views. Recording it replaces the list's compute root
/// signature and pipeline: the caller sets its own again.
/// </summary>
internal static unsafe class UavFill
{
    private const string Source = """
        #define RS "RootConstants(num32BitConstants=8, b0), " \
            "DescriptorTable(UAV(u0, space=1, numDescriptors=unbounded)), DescriptorTable(UAV(u0, space=2, numDescriptors=unbounded)), " \
            "DescriptorTable(UAV(u0, space=3, numDescriptors=unbounded)), DescriptorTable(UAV(u0, space=4, numDescriptors=unbounded)), " \
            "DescriptorTable(UAV(u0, space=5, numDescriptors=unbounded)), DescriptorTable(UAV(u0, space=6, numDescriptors=unbounded))"
        cbuffer Fill : register(b0) { uint index; uint kind; uint width; uint height; uint value; float red; float green; float blue; };
        RWTexture2D<float4> floats2[] : register(u0, space1);
        RWTexture2D<uint> uints2[] : register(u0, space2);
        RWTexture2DArray<float4> floatArrays[] : register(u0, space3);
        RWTexture2DArray<uint> uintArrays[] : register(u0, space4);
        RWTexture3D<float4> floats3[] : register(u0, space5);
        RWTexture3D<uint> uints3[] : register(u0, space6);
        [RootSignature(RS)]
        [numthreads(8, 8, 1)]
        void main(uint3 id : SV_DispatchThreadID) {
            if (id.x >= width || id.y >= height) return;
            float4 colour = float4(red, green, blue, 1.0f);
            if (kind == 0u) floats2[index][id.xy] = colour;
            else if (kind == 1u) uints2[index][id.xy] = value;
            else if (kind == 2u) floatArrays[index][id] = colour;
            else if (kind == 3u) uintArrays[index][id] = value;
            else if (kind == 4u) floats3[index][id] = colour;
            else uints3[index][id] = value;
        }
        """;

    /// <summary>Root parameters: the constants, then one table per kind.</summary>
    private const uint ConstantsParameter = 0;
    private const uint Tables = 6;
    private const uint Constants = 8;
    private const uint GroupSide = 8;

    /// <summary>The program's first kind of 2D-array views and of 3D views (float, then uint).</summary>
    private const uint ArrayKinds = 2;
    private const uint VolumeKinds = 4;

    /// <summary>D3DCOMPILE_ENABLE_UNBOUNDED_DESCRIPTOR_TABLES: the program indexes whole heap tables.</summary>
    private const uint UnboundedTables = 1u << 20;

    /// <summary>ID3D12Device: CreateComputePipelineState and CreateRootSignature (d3d12.h).</summary>
    private const int CreateComputePipelineSlot = 11;
    private const int CreateRootSignatureSlot = 16;

    /// <summary>ID3D12GraphicsCommandList: Dispatch, SetPipelineState, SetComputeRootSignature, SetComputeRootDescriptorTable, SetComputeRoot32BitConstants.</summary>
    private const int DispatchSlot = 14;
    private const int SetPipelineSlot = 25;
    private const int SetRootSignatureSlot = 29;
    private const int SetTableSlot = 31;
    private const int SetConstantsSlot = 35;

    /// <summary>The DXGI *_UINT formats (a uint view); *_SINT and typeless views are not written.</summary>
    private static readonly HashSet<int> UintFormats = [3, 7, 12, 17, 25, 30, 36, 42, 50, 57, 62];
    private static readonly HashSet<int> SintFormats = [4, 8, 14, 18, 32, 38, 43, 52, 59, 64];

    private static readonly object Gate = new();
    private static readonly Guid RootSignatureIid = new("c54a6b66-72df-4ee8-8be5-a946a1429214");
    private static readonly Guid PipelineIid = new("765a30f3-f624-4c6f-a828-ace948622445");
    private static nint _root;
    private static nint _pipeline;
    private static bool _failed;

    /// <summary>Records the fill of the UAV at <paramref name="index"/> of the heap whose GPU start is <paramref name="heapStart"/>; false when the view is not written.</summary>
    public static bool Record(nint device, nint list, ulong heapStart, uint index, UavView view, FillValue fill)
    {
        uint kind = Kind(view);

        if (kind == uint.MaxValue || !Ready(device))
        {
            return false;
        }

        ((delegate* unmanaged<nint, nint, void>)ComObject.Slot(list, SetRootSignatureSlot))(list, _root);
        ((delegate* unmanaged<nint, nint, void>)ComObject.Slot(list, SetPipelineSlot))(list, _pipeline);

        for (uint table = 1; table <= Tables; table++)
        {
            ((delegate* unmanaged<nint, uint, ulong, void>)ComObject.Slot(list, SetTableSlot))(list, table, heapStart);
        }

        // The program's cbuffer: index, kind, width, height, the uint value, then the colour.
        uint* constants = stackalloc uint[]
        {
            index, kind, view.Width, view.Height, fill.Packed,
            BitConverter.SingleToUInt32Bits(fill.Red), BitConverter.SingleToUInt32Bits(fill.Green), BitConverter.SingleToUInt32Bits(fill.Blue),
        };
        ((delegate* unmanaged<nint, uint, uint, uint*, uint, void>)ComObject.Slot(list, SetConstantsSlot))(list, ConstantsParameter, Constants, constants, 0);

        uint depth = 1;

        if (view.Dimension != UavViews.Texture2D)
        {
            depth = view.Slices;
        }

        ((delegate* unmanaged<nint, uint, uint, uint, void>)ComObject.Slot(list, DispatchSlot))(list, (view.Width + GroupSide - 1) / GroupSide, (view.Height + GroupSide - 1) / GroupSide, depth);

        return true;
    }

    /// <summary>The program's kind of view: 0/1 2D float/uint, 2/3 array, 4/5 3D; uint.MaxValue for a view not written.</summary>
    public static uint Kind(UavView view)
    {
        if (SintFormats.Contains(view.Format) || view.Format == 0)
        {
            return uint.MaxValue;
        }

        uint uint32 = 0;

        if (UintFormats.Contains(view.Format))
        {
            uint32 = 1;
        }

        switch (view.Dimension)
        {
            case UavViews.Texture2D:
                return uint32;
            case UavViews.Texture2DArray:
                return ArrayKinds + uint32;
            case UavViews.Texture3D:
                return VolumeKinds + uint32;
            default:
                return uint.MaxValue;
        }
    }

    private static bool Ready(nint device)
    {
        if (_pipeline != 0)
        {
            return true;
        }

        lock (Gate)
        {
            if (_pipeline == 0 && !_failed)
            {
                _failed = !Make(device);
            }

            return _pipeline != 0;
        }
    }

    private static bool Make(nint device)
    {
        nint code = Compile();

        if (device == 0 || code == 0)
        {
            return false;
        }

        byte* bytes = ((delegate* unmanaged<nint, byte*>)ComObject.Slot(code, IndirectClampNative.BlobPointerSlot))(code);
        nuint size = ((delegate* unmanaged<nint, nuint>)ComObject.Slot(code, IndirectClampNative.BlobSizeSlot))(code);
        Guid rootIid = RootSignatureIid;
        nint root = 0;
        int hr = ((delegate* unmanaged<nint, uint, byte*, nuint, Guid*, nint*, int>)ComObject.Slot(device, CreateRootSignatureSlot))(device, 0, bytes, size, &rootIid, &root);

        if (!HResult.Failed(hr))
        {
            nint* description = stackalloc nint[] { root, (nint)bytes, (nint)size, 0, 0, 0, 0 };
            Guid pipelineIid = PipelineIid;
            nint pipeline = 0;
            hr = ((delegate* unmanaged<nint, nint*, Guid*, nint*, int>)ComObject.Slot(device, CreateComputePipelineSlot))(device, description, &pipelineIid, &pipeline);
            _pipeline = pipeline;
        }

        ComObject.Release(code);
        _root = root;

        if (HResult.Failed(hr))
        {
            ShimContext.Log?.Warn($"[d3d12_xs] stand-in UAV fill: no program ({HResult.Format(hr)}); held passes leave their outputs as they are");
        }

        return !HResult.Failed(hr);
    }

    /// <summary>The program compiled by d3dcompiler_47 (cs_5_1, its root signature inside); 0 when that fails.</summary>
    private static nint Compile()
    {
        byte[] source = Encoding.ASCII.GetBytes(Source);
        byte[] entry = Encoding.ASCII.GetBytes("main\0");
        byte[] target = Encoding.ASCII.GetBytes("cs_5_1\0");
        byte[] name = Encoding.ASCII.GetBytes("uav_fill\0");

        nint code = 0;
        nint errors = 0;
        int hr;

        fixed (byte* text = source, main = entry, profile = target, file = name)
        {
            hr = IndirectClampNative.D3DCompile(text, (nuint)source.Length, file, 0, 0, main, profile, UnboundedTables, 0, &code, &errors);
        }

        if (errors != 0)
        {
            byte* message = ((delegate* unmanaged<nint, byte*>)ComObject.Slot(errors, IndirectClampNative.BlobPointerSlot))(errors);
            nuint length = ((delegate* unmanaged<nint, nuint>)ComObject.Slot(errors, IndirectClampNative.BlobSizeSlot))(errors);
            ShimContext.Log?.Warn($"[d3d12_xs] stand-in UAV fill: {Encoding.ASCII.GetString(message, (int)length).Trim()}");
            ComObject.Release(errors);
        }

        if (HResult.Failed(hr))
        {
            return 0;
        }

        return code;
    }
}
