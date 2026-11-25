// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Shims.D3D12X.Resources;
using Xunit;

namespace FiveStar.Loader.Tests;

/// <summary>
/// Slot-70 views of the lighting inputs: a full-format G-buffer target keeps its own format, so a view in another member
/// of its family loses the sRGB decode or the integer reading; a typeless one takes the view's.
/// </summary>
public sealed class ViewFormatsTests
{
    /// <summary>DXGI_FORMAT values (dxgiformat.h).</summary>
    private const int Rgba8Typeless = 27;
    private const int Rgba8Unorm = 28;
    private const int Rgba8Srgb = 29;
    private const int R32Float = 41;
    private const int R32Uint = 42;
    private const int R8Unorm = 61;
    private const int R8Uint = 62;
    private const int D32Float = 40;

    [Theory]
    [InlineData(Rgba8Unorm, Rgba8Srgb, Rgba8Unorm, (int)ViewFormatLoss.Cast)]
    [InlineData(R8Unorm, R8Uint, R8Unorm, (int)ViewFormatLoss.Cast)]
    [InlineData(R32Float, R32Uint, R32Float, (int)ViewFormatLoss.Cast)]
    [InlineData(Rgba8Unorm, R32Uint, Rgba8Unorm, (int)ViewFormatLoss.Reinterpret)]
    [InlineData(Rgba8Typeless, Rgba8Srgb, Rgba8Srgb, (int)ViewFormatLoss.None)]
    [InlineData(Rgba8Unorm, Rgba8Unorm, Rgba8Unorm, (int)ViewFormatLoss.None)]
    [InlineData(D32Float, R32Float, ViewFormats.Unknown, (int)ViewFormatLoss.None)]
    public void A_view_reads_in_the_format_the_desktop_allows_and_names_what_it_loses(int texture, int view, int created, int loss)
    {
        int format = ViewFormats.ForView(texture, view);

        Assert.Equal(created, format);
        Assert.Equal((ViewFormatLoss)loss, ViewFormats.Loss(texture, view, format));
    }
}
