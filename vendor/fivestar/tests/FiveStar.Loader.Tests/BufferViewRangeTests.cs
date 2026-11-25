// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Shims.D3D12X.Resources;
using Xunit;

namespace FiveStar.Loader.Tests;

/// <summary>A structured view whose start is no whole number of elements into a shared buffer is moved as a raw view.</summary>
public sealed unsafe class BufferViewRangeTests
{
    // D3D12_UNORDERED_ACCESS_VIEW_DESC for a buffer: format, dimension, FirstElement, NumElements, stride, counter offset, flags.
    private const int UavFirst = 8;
    private const int UavCount = 16;
    private const int UavStride = 20;
    private const int UavCounter = 24;
    private const int Flags = 32;
    private const int Raw = 1;
    private const int R32Typeless = 39;

    [Fact]
    public void A_28_byte_view_at_a_64_KiB_offset_starts_at_its_own_byte_not_the_buffers_start()
    {
        byte* description = stackalloc byte[40];
        byte* copy = stackalloc byte[40];
        new Span<byte>(description, 40).Clear();
        *(int*)(description + 4) = 1;
        *(uint*)(description + UavCount) = 1000;
        *(uint*)(description + UavStride) = 28;

        const ulong Offset = 0x10000;
        Assert.True(BufferViewRanges.AsRaw(description, copy, Offset, 1000 * 28, true));

        Assert.Equal(R32Typeless, *(int*)copy);
        Assert.Equal(Offset / 4, *(ulong*)(copy + UavFirst));
        Assert.Equal(1000u * 28 / 4, *(uint*)(copy + UavCount));
        Assert.Equal(0u, *(uint*)(copy + UavStride));
        Assert.Equal(0ul, *(ulong*)(copy + UavCounter));
        Assert.Equal(Raw, *(int*)(copy + Flags) & Raw);

        // Off the 16 bytes a raw view's start must be on (D3D12_RAW_UAV_SRV_BYTE_ALIGNMENT), a dword included: no view at
        // all rather than an invalid one a driver may move.
        Assert.False(BufferViewRanges.AsRaw(description, copy, Offset + 2, 28, true));
        Assert.False(BufferViewRanges.AsRaw(description, copy, Offset + 4, 28, true));
        Assert.True(BufferViewRanges.AsRaw(description, copy, Offset + 16, 28, true));
    }
}
