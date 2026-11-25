// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using Vortice.Direct3D12;
using Vortice.DXGI;

namespace FiveStar.Shaders.Tests.Warp;

/// <summary>
/// A mesh-shader pipeline state built from a raw D3D12 pipeline-state stream: each subobject is a u32
/// D3D12_PIPELINE_STATE_SUBOBJECT_TYPE, its payload at its natural alignment, the whole subobject 8-byte aligned.
/// </summary>
public static class MeshPipelineState
{
    // D3D12_PIPELINE_STATE_SUBOBJECT_TYPE values.
    private const int RootSignatureType = 0;
    private const int PixelShaderType = 2;
    private const int BlendType = 8;
    private const int SampleMaskType = 9;
    private const int RasterizerType = 10;
    private const int DepthStencilType = 11;
    private const int TargetFormatsType = 15;
    private const int DepthFormatType = 16;
    private const int SampleDescriptionType = 17;
    private const int MeshShaderType = 25;
    private const int TargetSlots = 8;

    public static unsafe ID3D12PipelineState Create(ID3D12Device device, ID3D12RootSignature root, byte[] mesh, byte[] pixel, Format[] targets)
    {
        var meshHandle = GCHandle.Alloc(mesh, GCHandleType.Pinned);
        var pixelHandle = GCHandle.Alloc(pixel, GCHandleType.Pinned);

        try
        {
            var stream = new List<byte>();
            Add(stream, RootSignatureType, BitConverter.GetBytes((long)root.NativePointer), 8);
            Add(stream, MeshShaderType, Bytecode(meshHandle, mesh.Length), 8);
            Add(stream, PixelShaderType, Bytecode(pixelHandle, pixel.Length), 8);

            AddFixedState(stream, targets);
            Align(stream, 8);
            var bytes = stream.ToArray();

            fixed (byte* start = bytes)
            {
                using var device2 = device.QueryInterface<ID3D12Device2>();
                var description = new PipelineStateStreamDescription { SizeInBytes = (nuint)bytes.Length, SubObjectStream = (IntPtr)start };

                return device2.CreatePipelineState<ID3D12PipelineState>(description);
            }
        }
        finally
        {
            meshHandle.Free();
            pixelHandle.Free();
        }
    }

    private static void AddFixedState(List<byte> stream, Format[] targets)
    {
        // D3D12_RASTERIZER_DESC: solid, cull none, depth clip on, everything else off.
        Add(stream, RasterizerType, Words(3, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0), 4);

        // D3D12_DEPTH_STENCIL_DESC: depth and stencil off (valid enums in the unused fields).
        var depth = new List<byte>(Words(0, 0, 8, 0));
        depth.AddRange([0xFF, 0xFF, 0, 0]);
        depth.AddRange(Words(1, 1, 1, 8, 1, 1, 1, 8));
        Add(stream, DepthStencilType, [.. depth], 4);

        // D3D12_BLEND_DESC: per target blend off, ONE / ZERO / ADD, logic op NOOP, write mask 0xF.
        var blend = new List<byte>(Words(0, 0));

        for (var target = 0; target < TargetSlots; target++)
        {
            blend.AddRange(Words(0, 0, 2, 1, 1, 2, 1, 1, 4));
            blend.AddRange([0xF, 0, 0, 0]);
        }

        Add(stream, BlendType, [.. blend], 4);
        Add(stream, SampleMaskType, Words(-1), 4);
        var formats = new int[TargetSlots + 1];

        for (var target = 0; target < targets.Length; target++)
        {
            formats[target] = (int)targets[target];
        }

        formats[TargetSlots] = targets.Length;
        Add(stream, TargetFormatsType, Words(formats), 4);
        Add(stream, DepthFormatType, Words(0), 4);
        Add(stream, SampleDescriptionType, Words(1, 0), 4);
    }

    private static void Add(List<byte> stream, int type, byte[] payload, int alignment)
    {
        Align(stream, 8);
        stream.AddRange(BitConverter.GetBytes(type));
        Align(stream, alignment);
        stream.AddRange(payload);
    }

    private static void Align(List<byte> stream, int alignment)
    {
        while (stream.Count % alignment != 0)
        {
            stream.Add(0);
        }
    }

    private static byte[] Bytecode(GCHandle handle, int length)
    {
        return [.. BitConverter.GetBytes((long)handle.AddrOfPinnedObject()), .. BitConverter.GetBytes((long)length)];
    }

    private static byte[] Words(params int[] values)
    {
        return [.. values.SelectMany(BitConverter.GetBytes)];
    }
}
