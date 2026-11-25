// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Native;
using FiveStar.Loader.Shims.D3D12X.Binding;
using FiveStar.Loader.Shims.D3D12X.Resources;

namespace FiveStar.Loader.Shims.D3D12X;

/// <summary>
/// The D3D12.X descriptor slots that take no resource object (call sites in docs/d3d12x-shim.md). Slot 70 describes a
/// view by a D3D12_RESOURCE_DESC plus a 64-byte view desc (<see cref="AddressViewDescription"/>), resolved to a
/// desktop texture by <see cref="AddressShaderViews"/>. Slot 61 takes a 0x6c-byte console sampler desc
/// (<see cref="ConsoleSamplerDescription"/>), written as the desktop sampler <see cref="ConsoleSamplers"/> makes of it.
/// </summary>
internal static unsafe class XboxAddressDescriptors
{
    public const int CreateShaderResourceViewAtAddressSlot = 70;
    public const int CreateSamplerXSlot = 61;

    /// <summary>ID3D12Device::CreateSampler (d3d12.h).</summary>
    private const int DesktopSamplerSlot = 22;

    [UnmanagedCallersOnly]
    public static void CreateShaderResourceViewAtAddress(XboxObject* self, ResourceDescription* resource, AddressViewDescription* view, nint handle)
    {
        try
        {
            if (view == null)
            {
                return;
            }

            var record = ImageViewClasses.With(BufferRecords.Based(BufferRecords.ShaderResource(view), view->Address, BufferRecords.FirstElement(view, unordered: false), 0, ViewKind.AddressView), view);
            nint destination = D3D12X.DescriptorHeaps.Destination(handle, record);
            var texture = D3D12X.AddressViews.Create(self->Inner, resource, view, destination, record);
            D3D12X.DrawTrace?.AddressView(destination, resource, view, texture);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XboxAddressDescriptors.CreateShaderResourceViewAtAddress");
        }
    }

    [HotPath]
    [UnmanagedCallersOnly]
    public static void CreateSamplerX(XboxObject* self, ConsoleSamplerDescription* description, nint handle)
    {
        try
        {
            if (description == null)
            {
                return;
            }

            var sampler = ConsoleSamplers.ToDesktop(*description);
            nint inner = self->Inner;
            var create = (delegate* unmanaged<nint, DesktopSamplerDescription*, nint, void>)ComObject.Slot(inner, DesktopSamplerSlot);
            create(inner, &sampler, D3D12X.DescriptorHeaps.Destination(handle));
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XboxAddressDescriptors.CreateSamplerX");
        }
    }

    private static XboxDeviceFactory D3D12X => ShimContext.Runtime.D3D12X;
}
