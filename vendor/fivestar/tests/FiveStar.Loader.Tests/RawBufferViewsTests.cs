// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Shims.D3D12X.Binding;
using Xunit;

namespace FiveStar.Loader.Tests;

/// <summary>Kept-index buffer views a raw view cannot cover are null, counted by why.</summary>
public sealed unsafe class RawBufferViewsTests
{
    /// <summary>DXGI_FORMAT_R32_FLOAT: 4-byte elements, so element 1 starts at byte 4, off the 16-byte raw alignment.</summary>
    private const int R32Float = 41;

    [Fact]
    public void A_read_only_view_off_the_raw_alignment_is_null_and_counted_as_such()
    {
        var view = new BufferViewDescription { Format = R32Float, Dimension = BufferViewDescription.BufferDimension, SrvMapping = BufferViewDescription.DefaultMapping, SrvFirstElement = 1, SrvElements = 64 };
        var copy = default(BufferViewDescription);
        nint resource = 1;
        long before = RawBufferViews.MisalignedReadOnly;

        var written = (BufferViewDescription*)RawBufferViews.View(&view, (byte*)&copy, false, BufferRecord.None, ref resource);

        Assert.Equal(0, resource);
        Assert.Equal(0u, written->SrvElements);
        Assert.Equal(before + 1, RawBufferViews.MisalignedReadOnly);
        Assert.Contains("read-only and", RawBufferViews.NullSummary(), StringComparison.Ordinal);
    }
}
