// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Graphics.Pipelines;
using FiveStar.Graphics.Rendering;
using Vortice.Direct3D12;
using Vortice.DXGI;

namespace FiveStar.Graphics.Probes;

/// <summary>
/// Measures the hardware wave a mesh shader runs in: D3D12 reports only the range of every stage
/// (WaveLaneCountMin-Max), and a mesh program whose floor lies inside that range needs its superstep form unless the
/// driver runs mesh shaders wide enough. An RX 7800 XT reports 32-64 and runs every mesh shader in 64-lane waves, at
/// 32, 64 and 128 threads and with long loops alike. The probe (<see cref="Source"/>, mesh_wave_probe.hlsl, compiled
/// by the caller) dispatches groups of 128 threads, as the console's mesh programs have, and keeps the least and
/// greatest WaveGetLaneCount in a raw UAV at heap slot 0.
/// </summary>
public static class MeshWaveProbe
{
    /// <summary>The probe's profile.</summary>
    public const string Profile = "ms";

    private const string Resource = "FiveStar.Graphics.Probes.mesh_wave_probe.hlsl";
    private const int Groups = 16;
    private const int Words = 2;
    private const int Bytes = Words * sizeof(uint);

    // The least lane count starts at its greatest value, the greatest at 0.
    private const uint NoLeast = uint.MaxValue;

    /// <summary>The probe's HLSL, embedded in this assembly.</summary>
    public static string Source()
    {
        using var stream = typeof(MeshWaveProbe).Assembly.GetManifestResourceStream(Resource)
            ?? throw new InvalidOperationException($"{Resource} is not embedded");
        using var reader = new StreamReader(stream);

        return reader.ReadToEnd();
    }

    /// <summary>The narrowest wave the probe's groups ran in, or null when the probe could not run.</summary>
    public static int? Measure(ID3D12Device device, byte[] shader)
    {
        try
        {
            return Run(device, shader);
        }
        catch (SharpGen.Runtime.SharpGenException)
        {
            return null;
        }
        catch (InvalidOperationException)
        {
            return null;
        }
        catch (TimeoutException)
        {
            return null;
        }
    }

    private static int? Run(ID3D12Device device, byte[] shader)
    {
        using var root = device.CreateRootSignature(new RootSignatureDescription1(RootSignatureFlags.ConstantBufferViewShaderResourceViewUnorderedAccessViewHeapDirectlyIndexed, []));
        using var state = new PipelineBuilder(device, root).Mesh(new MeshProgram(shader, null, [], Format.Unknown));
        using var heap = device.CreateDescriptorHeap(new DescriptorHeapDescription(DescriptorHeapType.ConstantBufferViewShaderResourceViewUnorderedAccessView, 1, DescriptorHeapFlags.ShaderVisible));
        using var arena = new ResourceArena(device);
        using var context = new CommandContext(device);
        using var list = context.List.QueryInterface<ID3D12GraphicsCommandList6>();

        var start = arena.UploadBuffer(BitConverter.GetBytes((ulong)NoLeast));
        var lanes = arena.Own(device.CreateCommittedResource(
            new HeapProperties(HeapType.Default), HeapFlags.None, ResourceDescription.Buffer(Bytes, ResourceFlags.AllowUnorderedAccess), ResourceStates.CopyDest));
        var readback = arena.ReadbackBuffer(Bytes);
        device.CreateUnorderedAccessView(lanes, null, RawView(), heap.GetCPUDescriptorHandleForHeapStart());

        list.CopyBufferRegion(lanes, 0, start, 0, Bytes);
        list.ResourceBarrierTransition(lanes, ResourceStates.CopyDest, ResourceStates.UnorderedAccess);
        list.SetDescriptorHeaps([heap]);
        list.SetGraphicsRootSignature(root);
        list.SetPipelineState(state);

        list.DispatchMesh(Groups, 1, 1);
        list.ResourceBarrierTransition(lanes, ResourceStates.UnorderedAccess, ResourceStates.CopySource);
        list.CopyBufferRegion(readback, 0, lanes, 0, Bytes);
        context.ExecuteAndWait();

        var words = ResourceArena.Read(readback, Bytes);
        var least = BitConverter.ToInt32(words, 0);
        var greatest = BitConverter.ToInt32(words, sizeof(int));

        if (least <= 0 || greatest < least)
        {
            return null;
        }

        return least;
    }

    private static UnorderedAccessViewDescription RawView()
    {
        return new UnorderedAccessViewDescription
        {
            Format = Format.R32_Typeless,
            ViewDimension = UnorderedAccessViewDimension.Buffer,
            Buffer = new BufferUnorderedAccessView { FirstElement = 0, NumElements = Words, Flags = BufferUnorderedAccessViewFlags.Raw },
        };
    }
}
