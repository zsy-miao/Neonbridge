// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Collections.Concurrent;
using System.Text;
using FiveStar.Loader.Native;
using FiveStar.Loader.Shims.D3D12X.Pipelines;
using FiveStar.Loader.Shims.D3D12X.States;
using Vortice.Direct3D12;

namespace FiveStar.Loader.Shims.D3D12X.Resources;

/// <summary>
/// Writes a depth twin's values into its depth texture (<see cref="DepthTwins"/>): a full-screen triangle whose pixel
/// program reads the twin's UAV at its pixel and outputs it as SV_Depth, depth test ALWAYS with writes, no colour
/// targets. Recording it replaces the list's graphics root signature, pipeline, targets, viewport and topology: the
/// caller sets the game's again.
/// </summary>
internal static unsafe class DepthTwinCopy
{
    private const string Source = """
        RWTexture2D<float> twin : register(u0);
        float4 vs(uint id : SV_VertexID) : SV_Position {
            float2 corner = float2((id << 1) & 2, id & 2);
            return float4(corner * float2(2, -2) + float2(-1, 1), 0, 1);
        }
        [RootSignature("DescriptorTable(UAV(u0), visibility=SHADER_VISIBILITY_PIXEL)")]
        float ps(float4 position : SV_Position) : SV_Depth {
            return twin[uint2(position.xy)];
        }
        """;

    /// <summary>ID3D12GraphicsCommandList slots (d3d12.h).</summary>
    private const int DrawInstancedSlot = 12;
    private const int SetTopologySlot = 20;
    private const int SetViewportsSlot = 21;
    private const int SetScissorsSlot = 22;
    private const int SetPipelineSlot = 25;
    private const int SetGraphicsRootSignatureSlot = 30;
    private const int SetGraphicsTableSlot = 32;
    private const int SetRenderTargetsSlot = 46;

    /// <summary>D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST, D3D12_PRIMITIVE_TOPOLOGY_TYPE_TRIANGLE.</summary>
    private const int TriangleList = 4;
    private const int TriangleType = 3;

    /// <summary>D3D12_RESOURCE_STATE_DEPTH_WRITE.</summary>
    private const int DepthWrite = 0x10;

    private const int FullScreenVertices = 3;
    private const int SampleMaskAll = -1;

    private static readonly object Gate = new();
    private static readonly ConcurrentDictionary<int, nint> Pipelines = new();
    private static nint _root;
    private static byte[]? _vertex;
    private static byte[]? _pixel;
    private static bool _failed;

    /// <summary>
    /// Records the copy of <paramref name="twin"/> into its depth through the depth-stencil view at desktop handle
    /// <paramref name="view"/> in <paramref name="format"/>; the depth is moved from <paramref name="state"/> to
    /// DEPTH_WRITE and back. <paramref name="table"/> is the twin UAV's GPU handle in the bound view heap. False when the
    /// program could not be made.
    /// </summary>
    public static bool Record(nint device, nint list, DepthTwin twin, nint view, int format, int state, ulong table)
    {
        nint pipeline = Pipeline(device, format);

        if (pipeline == 0)
        {
            return false;
        }

        ResourceBarrierSlots.Uav(list, twin.Twin);
        Move(list, twin.Depth, state, DepthWrite);

        ((delegate* unmanaged<nint, nint, void>)ComObject.Slot(list, SetGraphicsRootSignatureSlot))(list, _root);
        ((delegate* unmanaged<nint, nint, void>)ComObject.Slot(list, SetPipelineSlot))(list, pipeline);
        ((delegate* unmanaged<nint, uint, ulong, void>)ComObject.Slot(list, SetGraphicsTableSlot))(list, 0, table);
        ((delegate* unmanaged<nint, uint, nint*, int, nint*, void>)ComObject.Slot(list, SetRenderTargetsSlot))(list, 0, null, 0, &view);

        float* viewport = stackalloc float[] { 0, 0, twin.Width, twin.Height, 0, 1 };
        int* scissor = stackalloc int[] { 0, 0, (int)twin.Width, (int)twin.Height };
        ((delegate* unmanaged<nint, uint, float*, void>)ComObject.Slot(list, SetViewportsSlot))(list, 1, viewport);
        ((delegate* unmanaged<nint, uint, int*, void>)ComObject.Slot(list, SetScissorsSlot))(list, 1, scissor);
        ((delegate* unmanaged<nint, int, void>)ComObject.Slot(list, SetTopologySlot))(list, TriangleList);
        ((delegate* unmanaged<nint, uint, uint, uint, uint, void>)ComObject.Slot(list, DrawInstancedSlot))(list, FullScreenVertices, 1, 0, 0);

        Move(list, twin.Depth, DepthWrite, state);

        return true;
    }

    private static void Move(nint list, nint resource, int before, int after)
    {
        if (before != after)
        {
            BarrierWriter.Transition(list, resource, before, after);
        }
    }

    /// <summary>The copy pipeline for depth-stencil views of <paramref name="format"/>; 0 when it cannot be made.</summary>
    private static nint Pipeline(nint device, int format)
    {
        if (Pipelines.TryGetValue(format, out var made))
        {
            return made;
        }

        lock (Gate)
        {
            if (Pipelines.TryGetValue(format, out made))
            {
                return made;
            }

            made = Make(device, format);
            Pipelines[format] = made;

            return made;
        }
    }

    private static nint Make(nint device, int format)
    {
        if (!Programs(device))
        {
            return 0;
        }

        nint device2 = 0;
        Guid device2Iid = DesktopDevice.Device2Iid;

        if (HResult.Failed(DesktopDevice.QueryInterface(device, &device2Iid, &device2)))
        {
            return 0;
        }

        try
        {
            using var stream = new DesktopStream();
            stream.AddPointer(PipelineStateSubObjectType.RootSignature, _root);
            stream.AddShader(PipelineStateSubObjectType.VertexShader, _vertex!);
            stream.AddShader(PipelineStateSubObjectType.PixelShader, _pixel!);

            stream.AddBytes(PipelineStateSubObjectType.Blend, StatePadding.Blend(DesktopStateBytes.DefaultBlend()));
            stream.AddWords(PipelineStateSubObjectType.SampleMask, [SampleMaskAll]);
            stream.AddBytes(PipelineStateSubObjectType.Rasterizer, Rasterizer());
            stream.AddBytes(PipelineStateSubObjectType.DepthStencil1, StatePadding.DepthStencil(DepthAlways()));
            stream.AddWords(PipelineStateSubObjectType.PrimitiveTopology, [TriangleType]);

            stream.AddWords(PipelineStateSubObjectType.RenderTargetFormats, new int[PipelineLayout.RenderTargetSlots + 1]);
            stream.AddWords(PipelineStateSubObjectType.DepthStencilFormat, [format]);
            stream.AddWords(PipelineStateSubObjectType.SampleDescription, [1, 0]);

            Guid iid = DesktopDevice.PipelineStateIid;
            nint pipeline = 0;
            int hr = stream.Create(device2, &iid, &pipeline);

            if (HResult.Failed(hr))
            {
                ShimContext.Log?.Warn($"[d3d12_xs] depth twins: no copy pipeline for depth format {format} ({HResult.Format(hr)})");

                return 0;
            }

            return pipeline;
        }
        finally
        {
            DesktopDevice.Release(device2);
        }
    }

    /// <summary>Solid, no culling, depth clip on.</summary>
    private static byte[] Rasterizer()
    {
        var rasterizer = DesktopStateBytes.DefaultRasterizer();
        BitConverter.TryWriteBytes(rasterizer.AsSpan(DesktopStateBytes.CullModeField), DesktopStateBytes.CullNone);

        return rasterizer;
    }

    /// <summary>Depth on with writes, test ALWAYS, stencil off.</summary>
    private static byte[] DepthAlways()
    {
        var depth = DesktopStateBytes.DefaultDepthStencil();
        BitConverter.TryWriteBytes(depth.AsSpan(DesktopStateBytes.DepthFuncField), DesktopStateBytes.ComparisonAlways);

        return depth;
    }

    /// <summary>The two programs and the root signature, compiled once by d3dcompiler_47.</summary>
    private static bool Programs(nint device)
    {
        if (_root != 0)
        {
            return true;
        }

        if (_failed)
        {
            return false;
        }

        _vertex = Compile("vs", "vs_5_1");
        _pixel = Compile("ps", "ps_5_1");
        nint root = 0;

        if (_vertex is null || _pixel is null)
        {
            _failed = true;

            return false;
        }

        fixed (byte* blob = _pixel)
        {
            _failed = HResult.Failed(DesktopDevice.CreateRootSignature(device, blob, (nuint)_pixel.Length, &root));
        }

        _root = root;

        return !_failed;
    }

    private static byte[]? Compile(string entryName, string profileName)
    {
        byte[] source = Encoding.ASCII.GetBytes(Source);
        byte[] entry = Encoding.ASCII.GetBytes(entryName + "\0");
        byte[] target = Encoding.ASCII.GetBytes(profileName + "\0");
        byte[] name = Encoding.ASCII.GetBytes("depth_twin_copy\0");

        nint code = 0;
        nint errors = 0;
        int hr;

        fixed (byte* text = source, main = entry, profile = target, file = name)
        {
            hr = IndirectClampNative.D3DCompile(text, (nuint)source.Length, file, 0, 0, main, profile, 0, 0, &code, &errors);
        }

        if (errors != 0)
        {
            byte* message = ((delegate* unmanaged<nint, byte*>)ComObject.Slot(errors, IndirectClampNative.BlobPointerSlot))(errors);
            nuint length = ((delegate* unmanaged<nint, nuint>)ComObject.Slot(errors, IndirectClampNative.BlobSizeSlot))(errors);
            ShimContext.Log?.Warn($"[d3d12_xs] depth twins: {Encoding.ASCII.GetString(message, (int)length).Trim()}");
            ComObject.Release(errors);
        }

        if (HResult.Failed(hr) || code == 0)
        {
            return null;
        }

        byte* bytes = ((delegate* unmanaged<nint, byte*>)ComObject.Slot(code, IndirectClampNative.BlobPointerSlot))(code);
        nuint size = ((delegate* unmanaged<nint, nuint>)ComObject.Slot(code, IndirectClampNative.BlobSizeSlot))(code);
        var program = new ReadOnlySpan<byte>(bytes, (int)size).ToArray();
        ComObject.Release(code);

        return program;
    }
}
