// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using System.Text;
using FiveStar.Graphics.Rendering;
using Vortice.Direct3D12;

namespace FiveStar.Loader.Tests.Support;

/// <summary>
/// Reads a 2D RGBA8 texture back on the GPU through one SRV descriptor: a compute shader (compiled with the system's
/// d3dcompiler_47.dll) loads every texel of a mip and stores it packed into a buffer the test maps.
/// </summary>
internal sealed unsafe class TextureProbe : IDisposable
{
    private const string Source = """
        Texture2D<float4> source : register(t0);
        RWByteAddressBuffer output : register(u0);
        cbuffer Constants : register(b0) { uint width; uint height; uint mip; };
        [numthreads(8, 8, 1)]
        void main(uint3 id : SV_DispatchThreadID)
        {
            if (id.x >= width || id.y >= height) { return; }
            uint4 v = (uint4)round(source.Load(int3(id.xy, mip)) * 255.0);
            output.Store((id.y * width + id.x) * 4, v.x | (v.y << 8) | (v.z << 16) | (v.w << 24));
        }
        """;

    private const int GroupSize = 8;
    private const int Constants = 3;
    private const int BlobPointerSlot = 3;
    private const int BlobSizeSlot = 4;

    private readonly ID3D12Device _device;
    private readonly ID3D12RootSignature _root;
    private readonly ID3D12PipelineState _pipeline;

    public TextureProbe(nint device)
    {
        Marshal.AddRef(device);
        _device = new ID3D12Device(device);
        Heap = _device.CreateDescriptorHeap(new DescriptorHeapDescription(DescriptorHeapType.ConstantBufferViewShaderResourceViewUnorderedAccessView, 1, DescriptorHeapFlags.ShaderVisible));
        DescriptorRange1[] ranges = [new(DescriptorRangeType.ShaderResourceView, 1, 0)];
        RootParameter1[] parameters =
        [
            new(new RootConstants(0, 0, Constants), ShaderVisibility.All),
            new(new RootDescriptorTable1(ranges), ShaderVisibility.All),
            new(RootParameterType.UnorderedAccessView, new RootDescriptor1(0, 0), ShaderVisibility.All),
        ];

        _root = _device.CreateRootSignature(new RootSignatureDescription1(RootSignatureFlags.None, parameters));
        _pipeline = _device.CreateComputePipelineState<ID3D12PipelineState>(new ComputePipelineStateDescription { RootSignature = _root, ComputeShader = Compile() });
    }

    /// <summary>The shader-visible heap whose first descriptor the probe reads.</summary>
    public ID3D12DescriptorHeap Heap { get; }

    /// <summary>The packed texels of <paramref name="mip"/> (width x height).</summary>
    public uint[] Read(uint width, uint height, uint mip)
    {
        ulong bytes = width * height * sizeof(uint);
        using var output = _device.CreateCommittedResource(new HeapProperties(HeapType.Default), HeapFlags.None, ResourceDescription.Buffer(bytes, ResourceFlags.AllowUnorderedAccess), ResourceStates.Common);
        using var readback = _device.CreateCommittedResource(new HeapProperties(HeapType.Readback), HeapFlags.None, ResourceDescription.Buffer(bytes), ResourceStates.CopyDest);
        using var context = new CommandContext(_device);
        var list = context.List;

        list.ResourceBarrierTransition(output, ResourceStates.Common, ResourceStates.UnorderedAccess);
        list.SetDescriptorHeaps(Heap);
        list.SetComputeRootSignature(_root);
        list.SetPipelineState(_pipeline);

        uint* constants = stackalloc uint[] { width, height, mip };
        list.SetComputeRoot32BitConstants(0, Constants, constants, 0);
        list.SetComputeRootDescriptorTable(1, Heap.GetGPUDescriptorHandleForHeapStart());
        list.SetComputeRootUnorderedAccessView(2, output.GPUVirtualAddress);
        list.Dispatch((width + GroupSize - 1) / GroupSize, (height + GroupSize - 1) / GroupSize, 1);

        list.ResourceBarrierTransition(output, ResourceStates.UnorderedAccess, ResourceStates.CopySource);
        list.CopyResource(readback, output);
        context.ExecuteAndWait();

        return MemoryMarshal.Cast<byte, uint>(ResourceArena.Read(readback, (int)bytes)).ToArray();
    }

    public void Dispose()
    {
        _pipeline.Dispose();
        _root.Dispose();
        Heap.Dispose();
        _device.Dispose();
    }

    [DllImport("d3dcompiler_47.dll", CharSet = CharSet.Ansi, BestFitMapping = false)]
    private static extern int D3DCompile(byte* source, nuint size, string? name, nint defines, nint include, string entry, string target, uint flags, uint effectFlags, out nint code, out nint errors);

    private static byte[] Compile()
    {
        byte[] text = Encoding.ASCII.GetBytes(Source);

        fixed (byte* source = text)
        {
            int hr = D3DCompile(source, (nuint)text.Length, null, 0, 0, "main", "cs_5_0", 0, 0, out nint code, out nint errors);

            if (hr < 0)
            {
                throw new InvalidOperationException($"compute shader: 0x{hr:x8} {Marshal.PtrToStringAnsi(BlobData(errors))}");
            }

            var bytes = new ReadOnlySpan<byte>((void*)BlobData(code), (int)BlobSize(code)).ToArray();
            Marshal.Release(code);

            return bytes;
        }
    }

    private static nint BlobData(nint blob)
    {
        return ((delegate* unmanaged<nint, nint>)(*(nint**)blob)[BlobPointerSlot])(blob);
    }

    private static nuint BlobSize(nint blob)
    {
        return ((delegate* unmanaged<nint, nuint>)(*(nint**)blob)[BlobSizeSlot])(blob);
    }
}
