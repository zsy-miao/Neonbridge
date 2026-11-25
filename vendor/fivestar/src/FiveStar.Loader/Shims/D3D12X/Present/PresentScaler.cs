// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using System.Text;
using FiveStar.Loader.Native;
using FiveStar.Loader.Shims.D3D12X.Pipelines;
using Vortice.Direct3D12;

namespace FiveStar.Loader.Shims.D3D12X.Present;

/// <summary>
/// Draws the plane into a back buffer of another size: black bars, then the plane's source rect bilinearly scaled into the
/// largest rectangle of its aspect ratio (<see cref="Letterbox"/>). One full-screen triangle, a pipeline per back-buffer
/// format, a shader-visible view per back buffer (rewritten each present, so a frame in flight keeps its own). Programs
/// are compiled by d3dcompiler_47 (5.1, the root signature inside). Desktop objects, called through d3d12.h slots.
/// </summary>
internal sealed unsafe class PresentScaler : IDisposable
{
    /// <summary>ID3D12Device CreateDescriptorHeap, GetDescriptorHandleIncrementSize, CreateShaderResourceView.</summary>
    private const int CreateDescriptorHeapSlot = 14;
    private const int GetDescriptorHandleIncrementSizeSlot = 15;
    private const int CreateShaderResourceViewSlot = 18;

    /// <summary>ID3D12DescriptorHeap GetCPU/GPUDescriptorHandleForHeapStart.</summary>
    private const int HeapCpuStartSlot = 9;
    private const int HeapGpuStartSlot = 10;

    /// <summary>ID3D12GraphicsCommandList slots.</summary>
    private const int DrawInstancedSlot = 12;
    private const int SetPrimitiveTopologySlot = 20;
    private const int SetViewportsSlot = 21;
    private const int SetScissorRectsSlot = 22;
    private const int SetPipelineStateSlot = 25;
    private const int ResourceBarrierSlot = 26;
    private const int SetDescriptorHeapsSlot = 28;
    private const int SetGraphicsRootSignatureSlot = 30;
    private const int SetGraphicsRootDescriptorTableSlot = 32;
    private const int SetGraphicsRoot32BitConstantsSlot = 36;
    private const int SetRenderTargetsSlot = 46;
    private const int ClearRenderTargetViewSlot = 48;

    /// <summary>Root parameters: the source rect as four constants, the plane's view table.</summary>
    private const uint SourceParameter = 0;
    private const uint ViewParameter = 1;
    private const uint SourceConstants = 4;

    /// <summary>D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV, D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE.</summary>
    private const int ViewHeap = 0;
    private const int ShaderVisible = 1;

    /// <summary>D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST.</summary>
    private const int TriangleList = 4;
    private const uint TriangleVertices = 3;

    /// <summary>D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE.</summary>
    private const int PixelShaderResourceState = 0x80;

    private const string Source = """
        #define RS "RootConstants(num32BitConstants=4, b0), DescriptorTable(SRV(t0)), StaticSampler(s0, filter=FILTER_MIN_MAG_MIP_LINEAR, addressU=TEXTURE_ADDRESS_CLAMP, addressV=TEXTURE_ADDRESS_CLAMP, addressW=TEXTURE_ADDRESS_CLAMP)"
        cbuffer Source : register(b0) { float2 origin; float2 extent; };
        Texture2D<float4> plane : register(t0);
        SamplerState linearClamp : register(s0);
        struct Vertex { float4 position : SV_Position; float2 uv : TEXCOORD0; };
        [RootSignature(RS)]
        Vertex vs(uint id : SV_VertexID) {
            float2 corner = float2((id << 1) & 2, id & 2);
            Vertex v;
            v.position = float4(corner * float2(2, -2) + float2(-1, 1), 0, 1);
            v.uv = origin + corner * extent;
            return v;
        }
        [RootSignature(RS)]
        float4 ps(Vertex v) : SV_Target { return plane.SampleLevel(linearClamp, v.uv, 0); }
        """;

    private static readonly Guid HeapId = new("8efb471d-616c-4f49-90f7-127bb763fa51");
    private static readonly float[] Black = [0f, 0f, 0f, 1f];

    private readonly nint _device;
    private readonly Dictionary<int, nint> _pipelines = [];
    private byte[]? _vertexProgram;
    private byte[]? _pixelProgram;
    private nint _root;
    private nint _heap;
    private nint _cpuStart;
    private ulong _gpuStart;
    private uint _viewSize;

    public PresentScaler(nint device)
    {
        _device = device;
    }

    /// <summary>Programs, root signature and view heap; false (logged by the caller) when any of them fails.</summary>
    public bool Create(out string failure)
    {
        _vertexProgram = Compile("vs", "vs_5_1", out failure);
        _pixelProgram = Compile("ps", "ps_5_1", out string pixelFailure);

        if (_vertexProgram is null || _pixelProgram is null)
        {
            failure += pixelFailure;

            return false;
        }

        nint root = 0;
        int hr;

        fixed (byte* code = _vertexProgram)
        {
            hr = DesktopDevice.CreateRootSignature(_device, code, (nuint)_vertexProgram.Length, &root);
        }

        if (HResult.Failed(hr))
        {
            failure = $"root signature {HResult.Format(hr)}";

            return false;
        }

        _root = root;
        hr = CreateHeap();
        failure = HResult.Failed(hr) ? $"view heap {HResult.Format(hr)}" : string.Empty;

        return !HResult.Failed(hr);
    }

    /// <summary>
    /// Records the scaled plane into back buffer <paramref name="index"/> (render-target view <paramref name="view"/>,
    /// <paramref name="width"/> x <paramref name="height"/>, in PRESENT). False, with nothing recorded, when no pipeline
    /// takes the format.
    /// </summary>
    [HotPath]
    public bool Record(nint list, uint index, nint back, nint view, uint width, uint height, int format, PlaneSource source)
    {
        nint pipeline = Pipeline(format);

        if (pipeline == 0)
        {
            return false;
        }

        WriteView(index, source);
        Transition(list, back, TransitionBarrier.PresentState, TransitionBarrier.RenderTargetState);
        Transition(list, source.Resource, source.State, PixelShaderResourceState);

        Bind(list, pipeline, view);
        SetSource(list, index, source);
        Draw(list, Letterbox.Fit(source.Width, source.Height, width, height));

        Transition(list, source.Resource, PixelShaderResourceState, source.State);
        Transition(list, back, TransitionBarrier.RenderTargetState, TransitionBarrier.PresentState);

        return true;
    }

    /// <summary>Clears the back buffer black and binds it with the pipeline, root signature and view heap.</summary>
    [HotPath]
    private void Bind(nint list, nint pipeline, nint view)
    {
        fixed (float* black = Black)
        {
            ((delegate* unmanaged<nint, nint, float*, uint, PlaneRect*, void>)ComObject.Slot(list, ClearRenderTargetViewSlot))(list, view, black, 0, null);
        }

        nint heap = _heap;
        ((delegate* unmanaged<nint, uint, nint*, int, nint*, void>)ComObject.Slot(list, SetRenderTargetsSlot))(list, 1, &view, 0, null);
        ((delegate* unmanaged<nint, uint, nint*, void>)ComObject.Slot(list, SetDescriptorHeapsSlot))(list, 1, &heap);
        ((delegate* unmanaged<nint, nint, void>)ComObject.Slot(list, SetGraphicsRootSignatureSlot))(list, _root);
        ((delegate* unmanaged<nint, nint, void>)ComObject.Slot(list, SetPipelineStateSlot))(list, pipeline);
    }

    /// <summary>The plane's view and its source rect in texture coordinates.</summary>
    [HotPath]
    private void SetSource(nint list, uint index, PlaneSource source)
    {
        var rect = new SourceRect
        {
            Left = (float)source.Box.Left / source.ResourceWidth,
            Top = (float)source.Box.Top / source.ResourceHeight,
            Width = (float)source.Width / source.ResourceWidth,
            Height = (float)source.Height / source.ResourceHeight,
        };

        ((delegate* unmanaged<nint, uint, uint, SourceRect*, uint, void>)ComObject.Slot(list, SetGraphicsRoot32BitConstantsSlot))(list, SourceParameter, SourceConstants, &rect, 0);
        ((delegate* unmanaged<nint, uint, ulong, void>)ComObject.Slot(list, SetGraphicsRootDescriptorTableSlot))(list, ViewParameter, _gpuStart + (index * _viewSize));
    }

    /// <summary>The full-screen triangle, clipped to <paramref name="fit"/>.</summary>
    [HotPath]
    private static void Draw(nint list, Letterbox fit)
    {
        var viewport = new TargetViewport { X = fit.X, Y = fit.Y, Width = fit.Width, Height = fit.Height, MaxDepth = 1f };
        var scissor = new PlaneRect { Left = fit.X, Top = fit.Y, Right = fit.X + fit.Width, Bottom = fit.Y + fit.Height };
        ((delegate* unmanaged<nint, uint, TargetViewport*, void>)ComObject.Slot(list, SetViewportsSlot))(list, 1, &viewport);
        ((delegate* unmanaged<nint, uint, PlaneRect*, void>)ComObject.Slot(list, SetScissorRectsSlot))(list, 1, &scissor);
        ((delegate* unmanaged<nint, int, void>)ComObject.Slot(list, SetPrimitiveTopologySlot))(list, TriangleList);
        ((delegate* unmanaged<nint, uint, uint, uint, uint, void>)ComObject.Slot(list, DrawInstancedSlot))(list, TriangleVertices, 1, 0, 0);
    }

    public void Dispose()
    {
        foreach (nint pipeline in _pipelines.Values)
        {
            ComObject.Release(pipeline);
        }

        ComObject.Release(_root);
        ComObject.Release(_heap);
    }

    [HotPath]
    private static void Transition(nint list, nint resource, int before, int after)
    {
        if (before == after)
        {
            return;
        }

        var barrier = TransitionBarrier.Of(resource, before, after);
        ((delegate* unmanaged<nint, uint, TransitionBarrier*, void>)ComObject.Slot(list, ResourceBarrierSlot))(list, 1, &barrier);
    }

    [HotPath]
    private void WriteView(uint index, PlaneSource source)
    {
        var description = new ShaderViewDescription
        {
            Format = source.ViewFormat,
            Dimension = ShaderViewDescription.Texture2D,
            Mapping = ShaderViewDescription.DefaultMapping,
            MipLevels = 1,
        };

        var create = (delegate* unmanaged<nint, nint, ShaderViewDescription*, nint, void>)ComObject.Slot(_device, CreateShaderResourceViewSlot);
        create(_device, source.Resource, &description, _cpuStart + (nint)(index * _viewSize));
    }

    /// <summary>The pipeline drawing into <paramref name="format"/>, made on first use; 0 when that fails.</summary>
    private nint Pipeline(int format)
    {
        if (_pipelines.TryGetValue(format, out nint pipeline))
        {
            return pipeline;
        }

        nint device2 = 0;
        Guid device2Id = DesktopDevice.Device2Iid;

        if (!HResult.Failed(DesktopDevice.QueryInterface(_device, &device2Id, &device2)))
        {
            using var stream = Stream(format);
            Guid pipelineId = DesktopDevice.PipelineStateIid;
            _ = stream.Create(device2, &pipelineId, &pipeline);
            DesktopDevice.Release(device2);
        }

        _pipelines[format] = pipeline;

        return pipeline;
    }

    /// <summary>The pipeline stream: both programs, no culling, no depth, one target in <paramref name="format"/>.</summary>
    private DesktopStream Stream(int format)
    {
        var stream = new DesktopStream();
        stream.AddPointer(PipelineStateSubObjectType.RootSignature, _root);
        stream.AddShader(PipelineStateSubObjectType.VertexShader, _vertexProgram!);
        stream.AddShader(PipelineStateSubObjectType.PixelShader, _pixelProgram!);

        var rasterizer = DesktopStateBytes.DefaultRasterizer();
        BitConverter.TryWriteBytes(rasterizer.AsSpan(DesktopStateBytes.CullModeField), DesktopStateBytes.CullNone);
        stream.AddBytes(PipelineStateSubObjectType.Rasterizer, rasterizer);

        var depth = DesktopStateBytes.DefaultDepthStencil();
        DesktopStateBytes.Write(depth, DesktopStateBytes.DepthEnableField, false);
        BitConverter.TryWriteBytes(depth.AsSpan(DesktopStateBytes.DepthWriteMaskField), 0);
        stream.AddBytes(PipelineStateSubObjectType.DepthStencil1, depth);

        int[] targets = new int[PipelineLayout.RenderTargetSlots + 1];
        targets[0] = format;
        targets[PipelineLayout.RenderTargetSlots] = 1;
        stream.AddWords(PipelineStateSubObjectType.PrimitiveTopology, [PipelineLayout.TriangleTopology]);
        stream.AddWords(PipelineStateSubObjectType.RenderTargetFormats, targets);

        return stream;
    }

    private int CreateHeap()
    {
        var description = new DescriptorHeapDescription { Type = ViewHeap, Count = DxgiSwapChain.BufferCount, Flags = ShaderVisible };
        Guid heapId = HeapId;
        nint heap = 0;
        int hr = ((delegate* unmanaged<nint, DescriptorHeapDescription*, Guid*, nint*, int>)ComObject.Slot(_device, CreateDescriptorHeapSlot))(_device, &description, &heapId, &heap);

        if (HResult.Failed(hr))
        {
            return hr;
        }

        _heap = heap;
        nint cpu = 0;
        ulong gpu = 0;
        _ = ((delegate* unmanaged<nint, nint*, nint*>)ComObject.Slot(heap, HeapCpuStartSlot))(heap, &cpu);
        _ = ((delegate* unmanaged<nint, ulong*, ulong*>)ComObject.Slot(heap, HeapGpuStartSlot))(heap, &gpu);

        _cpuStart = cpu;
        _gpuStart = gpu;
        _viewSize = ((delegate* unmanaged<nint, int, uint>)ComObject.Slot(_device, GetDescriptorHandleIncrementSizeSlot))(_device, ViewHeap);

        return HResult.Ok;
    }

    /// <summary>One entry point of <see cref="Source"/>; null, with the compiler's message, when it does not compile.</summary>
    private static byte[]? Compile(string entryPoint, string profile, out string failure)
    {
        byte[] source = Encoding.ASCII.GetBytes(Source);
        byte[] entry = Encoding.ASCII.GetBytes(entryPoint + "\0");
        byte[] target = Encoding.ASCII.GetBytes(profile + "\0");
        byte[] name = Encoding.ASCII.GetBytes("present_scaler\0");

        nint code = 0;
        nint errors = 0;
        int hr;

        fixed (byte* text = source, main = entry, model = target, file = name)
        {
            hr = IndirectClampNative.D3DCompile(text, (nuint)source.Length, file, 0, 0, main, model, 0, 0, &code, &errors);
        }

        failure = string.Empty;

        if (errors != 0)
        {
            failure = Encoding.ASCII.GetString(Blob(errors)).Trim();
            ComObject.Release(errors);
        }

        if (HResult.Failed(hr) || code == 0)
        {
            failure = $"{profile} {HResult.Format(hr)} {failure}";

            return null;
        }

        byte[] program = Blob(code);
        ComObject.Release(code);

        return program;
    }

    private static byte[] Blob(nint blob)
    {
        byte* bytes = ((delegate* unmanaged<nint, byte*>)ComObject.Slot(blob, IndirectClampNative.BlobPointerSlot))(blob);
        nuint size = ((delegate* unmanaged<nint, nuint>)ComObject.Slot(blob, IndirectClampNative.BlobSizeSlot))(blob);

        return new ReadOnlySpan<byte>(bytes, (int)size).ToArray();
    }

    /// <summary>The root constants: the source rect's origin and size in texture coordinates.</summary>
    [StructLayout(LayoutKind.Sequential)]
    private struct SourceRect
    {
        public float Left;
        public float Top;
        public float Width;
        public float Height;
    }

    /// <summary>D3D12_VIEWPORT (d3d12.h).</summary>
    [StructLayout(LayoutKind.Sequential)]
    private struct TargetViewport
    {
        public float X;
        public float Y;
        public float Width;
        public float Height;
        public float MinDepth;
        public float MaxDepth;
    }

    /// <summary>D3D12_SHADER_RESOURCE_VIEW_DESC for a TEXTURE2D view (d3d12.h): 16 bytes, then the 24-byte union.</summary>
    [StructLayout(LayoutKind.Explicit, Size = 40)]
    private struct ShaderViewDescription
    {
        /// <summary>D3D12_SRV_DIMENSION_TEXTURE2D; D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING.</summary>
        public const int Texture2D = 4;
        public const uint DefaultMapping = 0x1688;

        [FieldOffset(0)]
        public int Format;

        [FieldOffset(4)]
        public int Dimension;

        [FieldOffset(8)]
        public uint Mapping;

        [FieldOffset(20)]
        public uint MipLevels;
    }
}
