// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using Vortice.Direct3D;
using Vortice.Direct3D12;
using Vortice.DXGI;

namespace FiveStar.Shaders.Tests.Warp;

/// <summary>Runs compute, graphics, mesh and mesh-fallback jobs on a WARP device and reads their results back.</summary>
public sealed class WarpRunner
{
    // The fallback's u0..u2 / t0 descriptors sit at heap slots 60..63 (after the slots tests use).
    private const int FallbackSlot = 60;
    private const int CounterBytes = 256;
    private const int DrawIndexedStride = 20;

    private readonly WarpDevice _device;
    private readonly UserDataBinding _userData;

    /// <summary>A runner that binds the user SGPRs as <paramref name="userData"/> says.</summary>
    public WarpRunner(WarpDevice device, UserDataBinding userData)
    {
        _device = device;
        _userData = userData;
    }

    public WarpResult Compute(ComputeJob job)
    {
        return Run(recording =>
        {
            var layout = RootLayout.Bindless;

            if (job.FallbackTable is not null)
            {
                layout = RootLayout.FallbackTable;
            }

            var root = recording.Own(PipelineFactory.RootSignature(_device.Device, job.Bindings.RootConstantCount, layout, _userData));
            var state = recording.Own(PipelineFactory.Compute(_device.Device, root, job.Shader));
            recording.CreateHeaps(job.Bindings);
            recording.SetComputeRoot(root, job.Bindings);

            if (job.FallbackTable is not null)
            {
                recording.List.SetComputeRootDescriptorTable(1, recording.ResourceTable(job.FallbackTable.Value));
            }

            recording.List.SetPipelineState(state);

            recording.List.Dispatch((uint)job.GroupsX, (uint)job.GroupsY, (uint)job.GroupsZ);
        });
    }

    public WarpResult Graphics(GraphicsJob job)
    {
        return Run(recording =>
        {
            var root = recording.Own(PipelineFactory.RootSignature(_device.Device, job.Bindings.RootConstantCount, RootLayout.Bindless, _userData));
            recording.CreateHeaps(job.Bindings);
            var formats = recording.CreateTargets(job.Framebuffer);
            var state = recording.Own(PipelineFactory.Graphics(_device.Device, root, job, formats));
            recording.SetGraphicsRoot(root, job.Bindings);
            recording.List.SetPipelineState(state);

            for (var slot = 0; slot < job.VertexBuffers.Count; slot++)
            {
                var stream = job.VertexBuffers[slot];
                var buffer = recording.Buffer(stream.Words.Length * 4, HeapType.Upload, ResourceStates.GenericRead);
                Recording.Write(buffer, stream.Words);
                recording.List.IASetVertexBuffers((uint)slot, new VertexBufferView(buffer.GPUVirtualAddress, (uint)stream.Words.Length * 4, (uint)stream.Stride));
            }

            var topology = job.Points ? PrimitiveTopology.PointList : PrimitiveTopology.TriangleList;
            recording.List.IASetPrimitiveTopology(topology);
            recording.BeginStatistics();
            recording.List.DrawInstanced((uint)job.VertexCount, (uint)job.InstanceCount, 0, 0);
            recording.EndStatistics();
        });
    }

    public WarpResult Mesh(MeshJob job)
    {
        return Run(recording =>
        {
            var root = recording.Own(PipelineFactory.RootSignature(_device.Device, job.Bindings.RootConstantCount, RootLayout.Bindless, _userData));
            recording.CreateHeaps(job.Bindings);
            var formats = recording.CreateTargets(job.Framebuffer);
            var state = recording.Own(MeshPipelineState.Create(_device.Device, root, job.MeshShader, job.PixelShader, formats));
            recording.SetGraphicsRoot(root, job.Bindings);
            recording.List.SetPipelineState(state);

            using var list6 = recording.List.QueryInterface<ID3D12GraphicsCommandList6>();
            recording.BeginStatistics();
            list6.DispatchMesh((uint)job.Groups, 1, 1);
            recording.EndStatistics();
        });
    }

    /// <summary>
    /// The fallback contract: clear u2 to {0, 1, 0, 0, 0, 0}; Dispatch; transition the outputs; ExecuteIndirect one
    /// DrawIndexed with arguments u2 and index buffer u1 (R32_UINT).
    /// </summary>
    public WarpResult MeshFallback(MeshFallbackJob job)
    {
        return Run(recording =>
        {
            var layout = job.RootDescriptors ? RootLayout.FallbackRoot : RootLayout.FallbackTable;
            var root = recording.Own(PipelineFactory.RootSignature(_device.Device, job.Bindings.RootConstantCount, layout, _userData));
            recording.CreateHeaps(job.Bindings);
            var formats = recording.CreateTargets(job.Framebuffer);

            var compute = recording.Own(PipelineFactory.Compute(_device.Device, root, job.ComputeShader));
            var draw = recording.Own(PipelineFactory.Graphics(_device.Device, root, new GraphicsJob(job.VertexShader, job.PixelShader, job.Framebuffer, job.Bindings), formats));
            var buffers = FallbackBuffers(recording, job);

            recording.BeginStatistics();
            recording.SetComputeRoot(root, job.Bindings);
            BindFallback(recording, job, buffers, compute: true);
            recording.List.SetPipelineState(compute);
            recording.List.Dispatch((uint)job.Groups, 1, 1);

            Transition(recording, buffers.Vertices, ResourceStates.UnorderedAccess, ResourceStates.NonPixelShaderResource);
            Transition(recording, buffers.Indices, ResourceStates.UnorderedAccess, ResourceStates.IndexBuffer);
            Transition(recording, buffers.Counter, ResourceStates.UnorderedAccess, ResourceStates.IndirectArgument);
            recording.SetGraphicsRoot(root, job.Bindings);
            BindFallback(recording, job, buffers, compute: false);
            recording.List.SetPipelineState(draw);

            DrawIndirect(recording, job, buffers);
            recording.EndStatistics();
            Transition(recording, buffers.Counter, ResourceStates.IndirectArgument, ResourceStates.CopySource);
            recording.ReadArguments(buffers.Counter);
        });
    }

    private static (ID3D12Resource Vertices, ID3D12Resource Indices, ID3D12Resource Counter) FallbackBuffers(Recording recording, MeshFallbackJob job)
    {
        var flags = ResourceFlags.AllowUnorderedAccess;
        var vertices = recording.FilledBuffer([], job.VertexBytes, ResourceStates.UnorderedAccess, flags);
        var indices = recording.FilledBuffer([.. job.StaleIndices], job.IndexBytes, ResourceStates.UnorderedAccess, flags);
        var counter = recording.FilledBuffer([0, 1, 0, 0, 0, 0, 0, 0], CounterBytes, ResourceStates.UnorderedAccess, flags);

        if (!job.RootDescriptors)
        {
            recording.CreateRawUav(vertices, job.VertexBytes / 4, FallbackSlot);
            recording.CreateRawUav(indices, job.IndexBytes / 4, FallbackSlot + 1);
            recording.CreateRawUav(counter, CounterBytes / 4, FallbackSlot + 2);
            recording.CreateRawSrv(vertices, job.VertexBytes / 4, FallbackSlot + 3);
        }

        return (vertices, indices, counter);
    }

    private static void BindFallback(
        Recording recording, MeshFallbackJob job, (ID3D12Resource Vertices, ID3D12Resource Indices, ID3D12Resource Counter) buffers, bool compute)
    {
        var list = recording.List;

        if (!job.RootDescriptors && compute)
        {
            list.SetComputeRootDescriptorTable(1, recording.ResourceTable(FallbackSlot));
        }
        else if (!job.RootDescriptors)
        {
            list.SetGraphicsRootDescriptorTable(1, recording.ResourceTable(FallbackSlot));
        }
        else if (compute)
        {
            list.SetComputeRootUnorderedAccessView(1, buffers.Vertices.GPUVirtualAddress);
            list.SetComputeRootUnorderedAccessView(2, buffers.Indices.GPUVirtualAddress);
            list.SetComputeRootUnorderedAccessView(3, buffers.Counter.GPUVirtualAddress);
        }
        else
        {
            list.SetGraphicsRootShaderResourceView(4, buffers.Vertices.GPUVirtualAddress);
        }
    }

    private void DrawIndirect(
        Recording recording, MeshFallbackJob job, (ID3D12Resource Vertices, ID3D12Resource Indices, ID3D12Resource Counter) buffers)
    {
        var arguments = new IndirectArgumentDescription { Type = IndirectArgumentType.DrawIndexed };
        var description = new CommandSignatureDescription { ByteStride = DrawIndexedStride, IndirectArguments = [arguments] };
        var signature = recording.Own(_device.Device.CreateCommandSignature<ID3D12CommandSignature>(description, null));
        recording.List.IASetPrimitiveTopology(PrimitiveTopology.TriangleList);
        recording.List.IASetIndexBuffer(new IndexBufferView(buffers.Indices.GPUVirtualAddress, (uint)job.IndexBytes, Format.R32_UInt));
        recording.List.ExecuteIndirect(signature, 1, buffers.Counter, 0, null, 0);
    }

    private static void Transition(Recording recording, ID3D12Resource resource, ResourceStates before, ResourceStates after)
    {
        recording.List.ResourceBarrier(ResourceBarrier.BarrierTransition(resource, before, after));
    }

    private WarpResult Run(Action<Recording> record)
    {
        _device.Recover();
        using var recording = new Recording(_device.Device) { UserData = _userData };
        record(recording);

        return recording.Finish(_device);
    }
}
