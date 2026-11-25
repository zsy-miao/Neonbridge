// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Shims.D3D12X.Binding;
using Xunit;

namespace FiveStar.Loader.Tests;

/// <summary>--typed-image-views: a texture view's record carries its component class in dword 3, a buffer view's its format.</summary>
public sealed unsafe class ImageViewClassesTests
{
    /// <summary>DXGI formats: R8G8B8A8_UNORM, X32_TYPELESS_G8X24_UINT (the stencil plane), R8_UINT, R32_SINT; SRV dimension Texture2D.</summary>
    private const int Rgba8Unorm = 28;
    private const int StencilPlane = 22;
    private const int R8Uint = 62;
    private const int R32Sint = 43;
    private const int Texture2D = 4;

    [Fact]
    public void A_texture_views_record_names_its_class_and_a_buffer_views_keeps_its_format()
    {
        try
        {
            ImageViewClasses.Enabled = true;

            Assert.Equal(ImageViewClasses.Uint, Class(StencilPlane));
            Assert.Equal(ImageViewClasses.Uint, Class(R8Uint));
            Assert.Equal(ImageViewClasses.Sint, Class(R32Sint));
            Assert.Equal(ImageViewClasses.Float, Class(Rgba8Unorm));

            var buffer = new BufferRecord(16, 64, 0x123);
            var view = new BufferViewDescription { Format = R8Uint, Dimension = BufferViewDescription.BufferDimension };
            Assert.Equal(0x123u, ImageViewClasses.With(buffer, &view).Word3);

            ImageViewClasses.Enabled = false;
            Assert.Equal(ImageViewClasses.Float, Class(R8Uint));
        }
        finally
        {
            ImageViewClasses.Enabled = false;
        }
    }

    private static uint Class(int format)
    {
        int* description = stackalloc int[10];
        description[0] = format;
        description[1] = Texture2D;

        return ImageViewClasses.With(BufferRecord.None, description).Word3;
    }
}
