// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using FiveStar.Loader.Native;
using Vortice.Direct3D;
using Vortice.Direct3D12;
using Vortice.DXGI;

namespace FiveStar.Loader.Shims.D3D12X.Pipelines;

/// <summary>
/// <c>fivestar-loader --pipeline-host</c>: builds one pipeline on a device of its own on the loader's adapter, answers
/// the HRESULT and ends. It runs as the loader's own executable, so the driver's disk cache it fills is the one the
/// loader's create reads (an RTX 4090 built the 2.75 MB foliage mesh pipeline in 191 s in one process and in 0.06 s in
/// the next of the same executable). Its memory and time are limited by the loader (<see cref="PipelineHost"/>).
/// </summary>
internal static unsafe class PipelineHostServer
{
    private const int Success = 0;
    private const int Failure = 1;
    private const int TouchStride = 4096;
    private const int AllocationBytes = 64 << 20;

    public static int Run()
    {
        using var input = new BinaryReader(Console.OpenStandardInput());
        using var output = new BinaryWriter(Console.OpenStandardOutput());
        var request = PipelineHostProtocol.ReadRequest(input);
        int hr;

        try
        {
            hr = Work(request, input);
        }
        catch (OutOfMemoryException)
        {
            hr = HResult.OutOfMemory;
        }

        PipelineHostProtocol.WriteAnswer(output, hr);

        if (HResult.Failed(hr))
        {
            return Failure;
        }

        return Success;
    }

    /// <summary>The test allocation, or the pipeline's build.</summary>
    private static int Work(PipelineHostRequest request, BinaryReader input)
    {
        if (request.AllocateBytes > 0)
        {
            Allocate(request.AllocateBytes);

            return HResult.Ok;
        }

        return Build(request, input);
    }

    private static int Build(PipelineHostRequest request, BinaryReader input)
    {
        using var factory = DXGI.CreateDXGIFactory2<IDXGIFactory4>(false);
        using var adapter = factory.EnumAdapterByLuid<IDXGIAdapter1>(GpuMemoryBudget.AdapterLuid(request.AdapterLuid));
        using var device = D3D12.D3D12CreateDevice<ID3D12Device>(adapter, FeatureLevel.Level_12_0);
        nint root = 0;
        int hr;

        fixed (byte* blob = request.RootSignature)
        {
            hr = DesktopDevice.CreateRootSignature(device.NativePointer, blob, (nuint)request.RootSignature.Length, &root);
        }

        if (HResult.Failed(hr))
        {
            return hr;
        }

        using var stream = DesktopStream.Read(input, root);
        var device2Iid = DesktopDevice.Device2Iid;
        var pipelineIid = DesktopDevice.PipelineStateIid;
        nint device2 = 0;
        nint pipeline = 0;
        hr = DesktopDevice.QueryInterface(device.NativePointer, &device2Iid, &device2);

        if (!HResult.Failed(hr))
        {
            hr = stream.Create(device2, &pipelineIid, &pipeline);
            DesktopDevice.Release(device2);
        }

        if (pipeline != 0)
        {
            DesktopDevice.Release(pipeline);
        }

        DesktopDevice.Release(root);

        return hr;
    }

    /// <summary>Allocates and touches <paramref name="bytes"/> in pieces: the host past its memory limit fails or is ended.</summary>
    private static void Allocate(long bytes)
    {
        var pieces = new List<nint>();

        for (long done = 0; done < bytes; done += AllocationBytes)
        {
            var piece = (byte*)NativeMemory.Alloc(AllocationBytes);

            for (var offset = 0; offset < AllocationBytes; offset += TouchStride)
            {
                piece[offset] = 1;
            }

            pieces.Add((nint)piece);
        }

        pieces.ForEach(piece => NativeMemory.Free((void*)piece));
    }
}
