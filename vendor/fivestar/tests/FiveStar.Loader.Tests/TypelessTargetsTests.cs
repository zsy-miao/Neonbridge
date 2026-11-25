// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Native;
using FiveStar.Loader.Shims.D3D12X;
using FiveStar.Loader.Shims.D3D12X.Resources;
using FiveStar.Loader.Tests.Support;
using Xunit;

namespace FiveStar.Loader.Tests;

/// <summary>
/// --typeless-targets: a G-buffer target the game makes R8G8B8A8_UNORM is made typeless, so the lighting's
/// UNORM_SRGB view keeps its sRGB decode, and a view the game makes without a desc reads UNORM as before.
/// </summary>
public sealed unsafe class TypelessTargetsTests
{
    /// <summary>DXGI formats, resource and view dimensions and flags (dxgiformat.h, d3d12.h).</summary>
    private const int Rgba8Typeless = 27;
    private const int Rgba8Unorm = 28;
    private const int Rgba8Srgb = 29;
    private const int Texture2D = 3;
    private const int AllowRenderTarget = 0x1;
    private const int View2D = 4;
    private const int View2DArray = 5;
    private const int Layers = 6;
    private const int Size = 64;

    /// <summary>ID3D12Device::CreateCommittedResource (d3d12.h).</summary>
    private const int CreateCommittedResourceSlot = 27;
    private static readonly Guid ResourceIid = new("696442be-a72e-4059-bc79-5b5c98040fad");

    /// <summary>D3D12_TEX2D_ARRAY_SRV MipLevels and ArraySize: the union starts at dword 4, MostDetailedMip first.</summary>
    private const int SrvMipLevels = 5;
    private const int SrvArraySize = 7;

    /// <summary>D3D12_TEX2D_ARRAY_RTV ArraySize: the union starts at dword 2, MipSlice and FirstArraySlice first.</summary>
    private const int RtvArraySize = 4;

    [Fact]
    public void A_unorm_target_is_made_typeless_and_views_take_the_game_format_or_unorm_without_a_desc()
    {
        using var test = TestRuntime.Create();
        _ = new XboxDevice(test);
        nint device = test.Runtime.D3D12X.Objects.DesktopDevice;
        TypelessTargets.Enabled = true;

        try
        {
            var console = new ResourceDescription { Dimension = Texture2D, Width = Size, Height = Size, DepthOrArraySize = Layers, MipLevels = 1, Format = Rgba8Unorm, SampleCount = 1, Flags = AllowRenderTarget };
            var desktop = test.Runtime.D3D12X.Descriptions.ToDesktop(device, &console);
            nint target = Committed(device, desktop);

            byte* view = stackalloc byte[DepthViewDescriptions.ShaderResourceViewSize];
            int* shader = (int*)TargetViewDescriptions.ShaderResourceView(target, null, view);
            byte* rtv = stackalloc byte[TargetViewDescriptions.RenderTargetViewSize];
            int* render = (int*)TargetViewDescriptions.RenderTargetView(target, null, rtv);

            Assert.Equal(Rgba8Typeless, desktop.Format);
            Assert.Equal(Rgba8Srgb, ViewFormats.ForView(desktop.Format, Rgba8Srgb));
            Assert.Equal((Rgba8Unorm, View2DArray, -1, Layers), (shader[0], shader[1], shader[SrvMipLevels], shader[SrvArraySize]));
            Assert.Equal((Rgba8Unorm, View2DArray, Layers), (render[0], render[1], render[RtvArraySize]));
            ComObject.Release(target);
        }
        finally
        {
            TypelessTargets.Enabled = false;
        }

        var single = new ResourceDescription { Dimension = Texture2D, Width = Size, Height = Size, DepthOrArraySize = 1, MipLevels = 1, Format = Rgba8Unorm, SampleCount = 1, Flags = AllowRenderTarget };
        Assert.Equal(View2D, TypelessTargets.Dimension(single));
        Assert.Equal(Rgba8Unorm, test.Runtime.D3D12X.Descriptions.ToDesktop(device, &single).Format);
    }

    private static nint Committed(nint device, ResourceDescription desc)
    {
        int* properties = stackalloc int[] { 1, 0, 0, 0, 0 };
        Guid iid = ResourceIid;
        nint resource = 0;
        var create = (delegate* unmanaged<nint, int*, int, ResourceDescription*, int, void*, Guid*, nint*, int>)ComObject.Slot(device, CreateCommittedResourceSlot);

        Assert.Equal(HResult.Ok, create(device, properties, 0, &desc, 0, null, &iid, &resource));

        return resource;
    }
}
