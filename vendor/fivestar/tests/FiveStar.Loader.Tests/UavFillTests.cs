// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Graphics;
using FiveStar.Graphics.Rendering;
using FiveStar.Loader.Shims.D3D12X.Binding;
using FiveStar.Loader.Tests.Support;
using Vortice.Direct3D12;
using Vortice.DXGI;
using Xunit;

namespace FiveStar.Loader.Tests;

/// <summary>
/// The stand-in fill of held ray-tracing passes (RtStandIns) on WARP: one constant over a unorm shadow mask, a float
/// 2D-array and an R32_UINT view of R9G9B9E5 light, each found by heap index with the view UavViews keeps.
/// </summary>
public sealed unsafe class UavFillTests
{
    private const int Size = 8;

    [WarpFact]
    public void A_held_pass_outputs_read_the_stand_in_value_whatever_their_format()
    {
        using var gpu = GpuDevice.CreateWarp(debugLayer: false);
        var device = gpu.Device;
        using var heap = device.CreateDescriptorHeap(new Vortice.Direct3D12.DescriptorHeapDescription(DescriptorHeapType.ConstantBufferViewShaderResourceViewUnorderedAccessView, 3, DescriptorHeapFlags.ShaderVisible));
        using var context = new CommandContext(device);
        using var arena = new ResourceArena(device);

        var increment = device.GetDescriptorHandleIncrementSize(DescriptorHeapType.ConstantBufferViewShaderResourceViewUnorderedAccessView);
        (Format Format, ushort Slices, FillValue Fill)[] targets =
        [
            (Format.R8_UNorm, 1, new FillValue(1, 1, 1, uint.MaxValue)),
            (Format.R16G16B16A16_Float, 2, new FillValue(0, 0, 0, 0)),
            (Format.R32_UInt, 1, FillValue.Shared(RtStandIns.AmbientLevel)),
        ];
        var textures = new List<ID3D12Resource>();

        uint* words = stackalloc uint[6];
        var list = context.List;
        list.SetDescriptorHeaps(1, new[] { heap });

        for (uint index = 0; index < targets.Length; index++)
        {
            var (format, slices, fill) = targets[index];
            var description = Vortice.Direct3D12.ResourceDescription.Texture2D(format, Size, Size, slices, 1, flags: ResourceFlags.AllowUnorderedAccess);
            var texture = device.CreateCommittedResource(HeapType.Default, description, ResourceStates.UnorderedAccess);
            textures.Add(texture);

            var dimension = slices > 1 ? UnorderedAccessViewDimension.Texture2DArray : UnorderedAccessViewDimension.Texture2D;
            var view = new UnorderedAccessViewDescription { Format = format, ViewDimension = dimension };
            view.Texture2DArray.ArraySize = slices;
            device.CreateUnorderedAccessView(texture, null, view, heap.GetCPUDescriptorHandleForHeapStart() + (int)(index * increment));

            (words[0], words[1], words[4]) = ((uint)format, (uint)dimension, slices);
            var known = UavViews.Of(Resource(description), words);
            Assert.NotNull(known);
            Assert.True(UavFill.Record(device.NativePointer, list.NativePointer, heap.GetGPUDescriptorHandleForHeapStart().Ptr, index, known.Value, fill));
        }

        context.ExecuteAndWait();

        Assert.Equal(0xFFu, Read(device, context, arena, textures[0], 1)[0] & 0xFFu);
        Assert.All(Read(device, context, arena, textures[1], 2 * 4), half => Assert.Equal(0u, half & 0xFFFFu));
        Assert.Equal(FillValue.SharedExponent(0.6f, 0.6f, 0.6f), Read(device, context, arena, textures[2], 4)[0]);
    }

    [Fact]
    public void The_screen_probe_grey_packs_as_r9g9b9e5()
    {
        // 0.6 = 307 / 512: exponent 15 (bias 15, mantissa scale 2^-9), the same mantissa in each channel.
        Assert.Equal(307u | (307u << 9) | (307u << 18) | (15u << 27), FillValue.SharedExponent(0.6f, 0.6f, 0.6f));
    }

    private static Shims.D3D12X.ResourceDescription Resource(Vortice.Direct3D12.ResourceDescription description)
    {
        return new Shims.D3D12X.ResourceDescription
        {
            Dimension = (int)description.Dimension, Width = description.Width, Height = description.Height,
            DepthOrArraySize = description.DepthOrArraySize, MipLevels = description.MipLevels, Format = (int)description.Format,
        };
    }

    /// <summary>The first row of the texture's first slice: <paramref name="bytesPerTexel"/> bytes per texel, as dwords.</summary>
    private static uint[] Read(ID3D12Device device, CommandContext context, ResourceArena arena, ID3D12Resource texture, int bytesPerTexel)
    {
        const int Pitch = 256;

        var readback = arena.ReadbackBuffer(Pitch * Size);
        var footprint = new PlacedSubresourceFootPrint { Footprint = new SubresourceFootPrint(texture.Description.Format, Size, Size, 1, Pitch) };
        context.List.ResourceBarrierTransition(texture, ResourceStates.UnorderedAccess, ResourceStates.CopySource);
        context.List.CopyTextureRegion(new TextureCopyLocation(readback, footprint), 0, 0, 0, new TextureCopyLocation(texture, 0));
        context.List.ResourceBarrierTransition(texture, ResourceStates.CopySource, ResourceStates.UnorderedAccess);
        context.ExecuteAndWait();

        void* mapped;
        readback.Map(0, null, &mapped).CheckError();
        var data = (uint*)mapped;
        var row = new uint[Math.Max(1, Size * bytesPerTexel / 4)];
        new Span<uint>(data, row.Length).CopyTo(row);
        readback.Unmap(0);

        return row;
    }
}
