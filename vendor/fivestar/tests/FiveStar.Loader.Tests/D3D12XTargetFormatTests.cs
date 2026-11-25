// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Shims.D3D12X;
using FiveStar.Loader.Tests.Support;
using Xunit;

namespace FiveStar.Loader.Tests;

/// <summary>
/// A B4G4R4A4_UNORM render target, as the game makes one at 0x140344196, on the default adapter: the allocation query
/// answers the console format's footprint, the resource is made in the substitute where the GPU does not render to
/// the format, and a view in the console format clears it. With FIVESTAR_D3D12_DEBUG=1 and this class run alone, the
/// debug layer checks the calls.
/// </summary>
public sealed unsafe class D3D12XTargetFormatTests
{
    private const int AllocationInfoSlot = 25;
    private const int CreateCommittedResourceSlot = 27;
    private const int CreateDescriptorHeapSlot = 14;
    private const int CreateRenderTargetViewSlot = 20;
    private const int HeapCpuStartSlot = 9;
    private const int ClearRenderTargetViewSlot = 52;
    private const int CloseSlot = 9;
    private const int ExecuteSlot = 35;
    private const int SignalSlot = 16;
    private const int FenceWaitSlot = 9;
    private const int GetDescSlot = 10;

    private const int DefaultHeap = 1;
    private const int Texture2D = 3;
    private const int RenderTargetFlag = 0x1;
    private const int RenderTargetState = 0x4;
    private const int RtvHeapType = 2;
    private const int RtvTexture2D = 4;
    private const uint Width = 2560;
    private const uint Height = 1440;
    private const ulong ConsoleBytesPerTexel = 2;
    private const ulong SubstituteBytesPerTexel = 4;
    private const int DefaultMapping = 0x1688;
    private const int OpaqueMapping = 0x1A88;
    private const string DebugVariable = "FIVESTAR_D3D12_DEBUG";
    private static readonly Guid ResourceIid = new("696442be-a72e-4059-bc79-5b5c98040fad");
    private static readonly Guid HeapIid = new("8efb471d-616c-4f49-90f7-127bb763fa51");

    [Fact]
    public void A_B4G4R4A4_render_target_is_sized_in_its_console_format_and_clears_through_a_console_view()
    {
        var log = new StringWriter();
        using var test = TestRuntime.Create(log: log);

        if (Environment.GetEnvironmentVariable(DebugVariable) == "1")
        {
            test.Runtime.D3D12X.Debugging.Enable(false);
        }

        var device = new XboxDevice(test);
        var desc = Description();
        int expected = TargetFormats.GpuWritten(test.Runtime.D3D12X.Objects.Unwrap(device.Device), TargetFormats.B4G4R4A4Unorm);

        ResourceAllocationInfo info;
        var query = (delegate* unmanaged<ResourceAllocationInfo*, nint, uint, uint, ResourceDescription*, ResourceAllocationInfo*>)XboxDevice.Vtable(device.Device)[AllocationInfoSlot];
        query(&info, device.Device, 0, 1, &desc);
        ulong texels = (ulong)Width * Height;
        Assert.InRange(info.Size, texels * ConsoleBytesPerTexel, (texels * SubstituteBytesPerTexel) - 1);

        nint target = Target(device, &desc);
        ResourceDescription created;
        nint inner = test.Runtime.D3D12X.Objects.Unwrap(target);
        _ = ((delegate* unmanaged<nint, ResourceDescription*, ResourceDescription*>)XboxDevice.Vtable(inner)[GetDescSlot])(inner, &created);
        Assert.Equal(expected, created.Format);

        Clear(device, View(device, target));
        Assert.DoesNotContain("[d3d12-debug] error", log.ToString(), StringComparison.Ordinal);
    }

    [Theory]
    [InlineData(TargetFormats.B5G6R5Unorm, DefaultMapping, OpaqueMapping)]
    [InlineData(TargetFormats.B4G4R4A4Unorm, DefaultMapping, DefaultMapping)]
    public void A_B5G6R5_view_of_the_substitute_reads_alpha_as_one(int format, int mapping, int expected)
    {
        Assert.Equal(expected, TargetFormats.ComponentMapping(format, mapping));
        Assert.Equal(TargetFormats.R8G8B8A8Unorm, TargetFormats.View(format, TargetFormats.R8G8B8A8Unorm));
    }

    private static ResourceDescription Description()
    {
        return new ResourceDescription
        {
            Dimension = Texture2D,
            Width = Width,
            Height = Height,
            DepthOrArraySize = 1,
            MipLevels = 1,
            Format = TargetFormats.B4G4R4A4Unorm,
            SampleCount = 1,
            Flags = RenderTargetFlag,
        };
    }

    /// <summary>Slot 27 with a clear value in the console format.</summary>
    private static nint Target(XboxDevice device, ResourceDescription* desc)
    {
        int* properties = stackalloc int[] { DefaultHeap, 0, 0, 0, 0 };
        float* clear = stackalloc float[] { 0, 0, 0, 0, 0 };
        *(int*)clear = TargetFormats.B4G4R4A4Unorm;

        Guid iid = ResourceIid;
        nint resource = 0;
        var create = (delegate* unmanaged<nint, int*, int, ResourceDescription*, int, void*, Guid*, nint*, int>)XboxDevice.Vtable(device.Device)[CreateCommittedResourceSlot];
        Assert.Equal(0, create(device.Device, properties, 0, desc, RenderTargetState, clear, &iid, &resource));

        return resource;
    }

    /// <summary>Slot 20 with a desc in the console format.</summary>
    private static nint View(XboxDevice device, nint resource)
    {
        var heapDesc = new DescriptorHeapDescription { Type = RtvHeapType, Count = 1 };
        Guid iid = HeapIid;
        nint heap = 0;
        var create = (delegate* unmanaged<nint, DescriptorHeapDescription*, Guid*, nint*, int>)XboxDevice.Vtable(device.Device)[CreateDescriptorHeapSlot];
        Assert.Equal(0, create(device.Device, &heapDesc, &iid, &heap));

        nint handle = ((delegate* unmanaged<nint, nint>)XboxDevice.Vtable(heap)[HeapCpuStartSlot])(heap);
        int* view = stackalloc int[] { TargetFormats.B4G4R4A4Unorm, RtvTexture2D, 0, 0, 0, 0 };
        ((delegate* unmanaged<nint, nint, void*, nint, void>)XboxDevice.Vtable(device.Device)[CreateRenderTargetViewSlot])(device.Device, resource, view, handle);

        return handle;
    }

    private static void Clear(XboxDevice device, nint view)
    {
        nint queue = device.Queue();
        nint fence = device.Fence();
        nint list = device.DirectList();
        float* color = stackalloc float[] { 0.25f, 0.5f, 0.75f, 1 };
        ((delegate* unmanaged<nint, nint, float*, uint, void*, void>)XboxDevice.Vtable(list)[ClearRenderTargetViewSlot])(list, view, color, 0, null);

        Assert.Equal(0, ((delegate* unmanaged<nint, int>)XboxDevice.Vtable(list)[CloseSlot])(list));
        nint* lists = stackalloc nint[] { list };
        Assert.Equal(0, ((delegate* unmanaged<nint, uint, nint*, nint, uint, int>)XboxDevice.Vtable(queue)[ExecuteSlot])(queue, 1, lists, 0, 0));
        Assert.Equal(0, ((delegate* unmanaged<nint, nint, ulong, int>)XboxDevice.Vtable(queue)[SignalSlot])(queue, fence, 1));
        Assert.Equal(0, ((delegate* unmanaged<nint, ulong, nint, int>)XboxDevice.Vtable(fence)[FenceWaitSlot])(fence, 1, 0));
    }
}
