// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Shims.D3D12X;
using Xunit;

namespace FiveStar.Loader.Tests;

/// <summary>Allocation info of console-layout textures: the XG size the game's archives were cut to (XgTests).</summary>
public sealed unsafe class ConsoleAllocationSizesTests
{
    private const int Texture2D = 3;
    private const int Bc1 = 71;
    private const int Small4KbRX = 0x100 + 23;
    private const int Rgba16Unorm = 11;
    private const int ConsoleFlags = 0x300000;
    private const int AllowRenderTarget = 1;
    private const int AllowDepthStencil = 2;
    private const int AllowUnorderedAccess = 4;
    private const int D16S8 = 118;
    private const uint Msaa4 = 4;
    private const ulong DepthTargetXgBytes = 0xb40000;
    private const int Rgba8Unorm = 28;
    private const ulong RenderTargetRegion = 64ul << 20;
    private const ulong RegionSlack = 2ul << 20;

    /// <summary>A 64 KiB block holds 128x128 four-byte elements (docs/xg.md); 3840x2160 takes 30 x 17 of them.</summary>
    private const ulong LargeBlockBytes = 0x10000;
    private const ulong Blocks4kTarget = 30 * 17;

    [Fact]
    public void A_console_layout_texture_gets_its_xg_size_and_a_buffer_keeps_the_desktop_answer()
    {
        var descs = stackalloc ResourceDescription[2];
        descs[0] = new ResourceDescription { Dimension = Texture2D, Width = 256, Height = 128, DepthOrArraySize = 1, MipLevels = 8, Format = Bc1, SampleCount = 1, Layout = Small4KbRX };
        descs[1] = new ResourceDescription { Dimension = ResourceDescription.BufferDimension, Width = 0x100, Height = 1, DepthOrArraySize = 1, MipLevels = 1, SampleCount = 1 };
        var infos = stackalloc ResourceAllocationInfo1[1];
        ResourceAllocationInfo result = default;

        Assert.True(ConsoleAllocationSizes.TryApply(descs, 1, &result, infos));
        Assert.Equal((0x6000ul, 0x1000ul), (result.Size, result.Alignment));
        Assert.Equal((0ul, 0x6000ul), (infos[0].Offset, infos[0].Size));

        result = default;
        Assert.False(ConsoleAllocationSizes.TryApply(descs, 2, &result, null));
        Assert.Equal(0ul, result.Size);
    }

    [Fact]
    public void A_streamed_texture_without_a_layout_gets_the_suggested_4_kb_blocks()
    {
        // The texture of the archive chunk at the pad pairing: 128x4 R16G16B16A16 is 0x2000 bytes on the desktop and
        // four 4 KB blocks (32x16 elements each) on the console, the 0x4000 bytes its compressed chunk holds.
        var desc = new ResourceDescription { Dimension = Texture2D, Width = 128, Height = 4, DepthOrArraySize = 1, MipLevels = 1, Format = Rgba16Unorm, SampleCount = 1, Flags = ConsoleFlags };
        ResourceAllocationInfo result = default;

        Assert.True(ConsoleAllocationSizes.TryApply(&desc, 1, &result, null));
        Assert.Equal(0x4000ul, result.Size);

        desc.Flags |= AllowRenderTarget;
        Assert.True(ConsoleAllocationSizes.TryApply(&desc, 1, &result, null));
        Assert.True(ConsoleAllocationSizes.TryCommittedTextureSize(&desc, out ulong targetSize));
        Assert.Equal(targetSize, result.Size);

        desc.SampleCount = Msaa4;
        Assert.False(ConsoleAllocationSizes.TryApply(&desc, 1, &result, null));
    }

    [Fact]
    public void A_depth_target_gets_its_xg_size_and_not_the_desktop_metadata()
    {
        // The 2560x1440 D16S8 depth targets the game places in its graphics heap: an RX 7800 XT answers 0x1330000
        // bytes, XG 0xb40000 (the heap ran out at the story load with the desktop sizes).
        var desc = new ResourceDescription { Dimension = Texture2D, Width = 2560, Height = 1440, DepthOrArraySize = 1, MipLevels = 1, Format = D16S8, SampleCount = 1, Flags = ConsoleFlags | AllowDepthStencil | AllowUnorderedAccess };
        ResourceAllocationInfo result = default;

        Assert.True(ConsoleAllocationSizes.TryApply(&desc, 1, &result, null));
        Assert.Equal(DepthTargetXgBytes, result.Size);
    }

    [Fact]
    public void Two_4k_render_targets_fill_the_region_the_game_hands_out_for_them()
    {
        // 0x14247ad10 creates two 3840x2160 RGBA8 targets from a 64 MiB region and breaks unless they fill all but
        // 2 MiB of it; an RDNA3 desktop driver asks 0x2010000 bytes for each, which overruns it.

        var desc = new ResourceDescription { Dimension = Texture2D, Width = 3840, Height = 2160, DepthOrArraySize = 1, MipLevels = 1, Format = Rgba8Unorm, SampleCount = 1, Flags = ConsoleFlags | AllowRenderTarget };

        Assert.True(ConsoleAllocationSizes.TryCommittedTextureSize(&desc, out ulong size));
        Assert.Equal(Blocks4kTarget * LargeBlockBytes, size);
        Assert.InRange(2 * size, RenderTargetRegion - RegionSlack, RenderTargetRegion);

        desc.Dimension = ResourceDescription.BufferDimension;
        Assert.False(ConsoleAllocationSizes.TryCommittedTextureSize(&desc, out _));
    }
}
