// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Graphics;
using FiveStar.Graphics.Rendering;
using FiveStar.Loader.Shims.D3D12X.Binding;
using FiveStar.Loader.Shims.D3D12X.Resources;
using FiveStar.Loader.Tests.Support;
using Vortice.Direct3D12;
using Vortice.DXGI;
using Xunit;

namespace FiveStar.Loader.Tests;

/// <summary>
/// --depth-twins on WARP: a compute pass writes the colour twin of a depth texture through its UAV, and the copy pass
/// puts those values in the depth, where a draw's depth test sees them.
/// </summary>
public sealed unsafe class DepthTwinTests
{
    private const int Size = 8;
    private const float Written = 0.25f;

    /// <summary>D3D12_RESOURCE_STATE_DEPTH_WRITE.</summary>
    private const int DepthWrite = 0x10;

    [WarpFact]
    public void A_value_written_to_the_twin_reaches_the_depth()
    {
        using var gpu = GpuDevice.CreateWarp(debugLayer: false);
        var device = gpu.Device;
        using var views = device.CreateDescriptorHeap(new Vortice.Direct3D12.DescriptorHeapDescription(DescriptorHeapType.ConstantBufferViewShaderResourceViewUnorderedAccessView, 1, DescriptorHeapFlags.ShaderVisible));
        using var targets = device.CreateDescriptorHeap(new Vortice.Direct3D12.DescriptorHeapDescription(DescriptorHeapType.DepthStencilView, 1));
        using var context = new CommandContext(device);
        using var arena = new ResourceArena(device);

        var description = Vortice.Direct3D12.ResourceDescription.Texture2D(Format.R32_Typeless, Size, Size, 1, 1, flags: ResourceFlags.AllowDepthStencil);
        using var depth = device.CreateCommittedResource(HeapType.Default, description, ResourceStates.DepthWrite, new ClearValue(Format.D32_Float, 1.0f, 0));
        device.CreateDepthStencilView(depth, new DepthStencilViewDescription { Format = Format.D32_Float, ViewDimension = DepthStencilViewDimension.Texture2D }, targets.GetCPUDescriptorHandleForHeapStart());

        var twin = DepthTwins.For(device.NativePointer, depth.NativePointer, Resource(description), DepthTwins.Float32);
        Assert.NotNull(twin);
        device.CreateUnorderedAccessView(Resource(twin.Twin), null, new UnorderedAccessViewDescription { Format = Format.R32_Float, ViewDimension = UnorderedAccessViewDimension.Texture2D }, views.GetCPUDescriptorHandleForHeapStart());

        var list = context.List;
        list.SetDescriptorHeaps(1, new[] { views });
        ulong table = views.GetGPUDescriptorHandleForHeapStart().Ptr;
        var view = new UavView((int)Format.R32_Float, UavViews.Texture2D, Size, Size, 1);

        Assert.True(UavFill.Record(device.NativePointer, list.NativePointer, table, 0, view, new FillValue(Written, Written, Written, 0)));
        Assert.True(DepthTwinCopy.Record(device.NativePointer, list.NativePointer, twin, (nint)targets.GetCPUDescriptorHandleForHeapStart().Ptr, (int)Format.D32_Float, DepthWrite, table));
        context.ExecuteAndWait();

        Assert.All(Read(context, arena, depth), value => Assert.Equal(Written, value));
    }

    [Fact]
    public void Only_the_depth_plane_formats_take_a_twin()
    {
        Assert.True(DepthTwins.Twinned(DepthTwins.Float32));
        Assert.True(DepthTwins.Twinned(DepthTwins.Unorm16));
        Assert.False(DepthTwins.Twinned((int)Format.R8_UInt));
    }

    private static ID3D12Resource Resource(nint pointer)
    {
        System.Runtime.InteropServices.Marshal.AddRef(pointer);

        return new ID3D12Resource(pointer);
    }

    private static Shims.D3D12X.ResourceDescription Resource(Vortice.Direct3D12.ResourceDescription description)
    {
        return new Shims.D3D12X.ResourceDescription
        {
            Dimension = (int)description.Dimension, Width = description.Width, Height = description.Height,
            DepthOrArraySize = description.DepthOrArraySize, MipLevels = description.MipLevels, Format = (int)description.Format,
        };
    }

    /// <summary>The first row of the depth as floats.</summary>
    private static float[] Read(CommandContext context, ResourceArena arena, ID3D12Resource depth)
    {
        const int Pitch = 256;

        var readback = arena.ReadbackBuffer(Pitch * Size);
        var footprint = new PlacedSubresourceFootPrint { Footprint = new SubresourceFootPrint(Format.R32_Typeless, Size, Size, 1, Pitch) };
        context.List.ResourceBarrierTransition(depth, ResourceStates.DepthWrite, ResourceStates.CopySource);
        context.List.CopyTextureRegion(new TextureCopyLocation(readback, footprint), 0, 0, 0, new TextureCopyLocation(depth, 0));
        context.ExecuteAndWait();

        void* mapped;
        readback.Map(0, null, &mapped).CheckError();
        var row = new float[Size];
        new Span<float>(mapped, Size).CopyTo(row);
        readback.Unmap(0);

        return row;
    }
}
