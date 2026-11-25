// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Shims.D3D12X;
using FiveStar.Loader.Tests.Support;
using Xunit;

namespace FiveStar.Loader.Tests;

/// <summary>D3D12XboxCreateDevice: the D3D12.X device vtable on a desktop device.</summary>
public sealed unsafe class D3D12XDeviceTests
{
    private const int GetNodeCountSlot = 7;
    private const int GetCustomHeapPropertiesSlot = 26;
    private const int UnmappedXboxSlot = 44;
    private const int CreateCommandQueueSlot = 48;
    private const int CreateCommandListSlot = 73;
    private const int CreateCommandAllocatorSlot = 9;
    private const int CloseSlot = 9;
    private const int ReleaseSlot = 2;
    private const int ComputeType = 2;
    private const int HeapTypeUpload = 2;
    private const int HeapTypeCustom = 4;
    private const int HeapPropertiesSize = 20;
    private const int NotImplemented = unchecked((int)0x80004001);
    private const int IncrementSizeSlot = 15;
    private const uint ViewHeapType = 0;
    private const uint ConsoleOnlyHeapType = 4;

    /// <summary>Desktop GetResourceAllocationInfo, DXGI_FORMAT_R8G8B8A8_TYPELESS and R32G8X24_TYPELESS, the last legacy DXGI format.</summary>
    private const int AllocationInfoSlot = 25;
    private const int Rgba8Typeless = 27;
    private const int DepthStencilTypeless = 19;
    private const int LastFormat = 115;
    private const int Texture2D = 3;
    private const uint TextureSize = 64;
    private const int AllowUnorderedAccess = 0x4;

    [Fact]
    public void Device_forwards_desktop_slots_and_stubs_xbox_slots()
    {
        using var test = TestRuntime.Create();
        var create = (delegate* unmanaged<nint, byte*, Guid*, nint*, int>)test.Shim("d3d12_xs.dll", "D3D12XboxCreateDevice");
        byte* parameters = stackalloc byte[0x50];
        var iid = Guid.Empty;
        nint device = 0;

        Assert.Equal(0, create(0, parameters, &iid, &device));
        nint* vtable = *(nint**)device;

        // A plain slot, then a struct-returning one: the game passes the return buffer first and the object second.
        uint nodes = ((delegate* unmanaged<nint, uint>)vtable[GetNodeCountSlot])(device);
        byte* properties = stackalloc byte[HeapPropertiesSize];
        var custom = (delegate* unmanaged<byte*, nint, uint, int, byte*>)vtable[GetCustomHeapPropertiesSlot];
        custom(properties, device, 1, HeapTypeUpload);
        int xboxOnly = ((delegate* unmanaged<nint, int>)vtable[UnmappedXboxSlot])(device);
        var increment = (delegate* unmanaged<nint, uint, uint>)vtable[IncrementSizeSlot];

        Assert.NotEqual(0u, increment(device, ViewHeapType));
        Assert.Equal(0u, increment(device, ConsoleOnlyHeapType));
        Assert.Equal(1u, nodes);
        Assert.Equal(HeapTypeCustom, *(int*)properties);
        Assert.Equal(NotImplemented, xboxOnly);
    }

    [Fact]
    public void Xbox_queue_and_list_slots_create_desktop_objects()
    {
        using var test = TestRuntime.Create();
        var create = (delegate* unmanaged<nint, byte*, Guid*, nint*, int>)test.Shim("d3d12_xs.dll", "D3D12XboxCreateDevice");
        byte* parameters = stackalloc byte[0x50];
        var iid = Guid.Empty;
        nint device = 0;

        Assert.Equal(0, create(0, parameters, &iid, &device));
        nint* vtable = *(nint**)device;

        // The game's compute-queue desc: type 2, flags 2 (not a desktop flag), node mask 0.
        int* queueDesc = stackalloc int[] { ComputeType, 0, 2, 0 };
        var queueIid = new Guid("0ec870a6-5d7e-4c22-8cfc-5baae07616ed");
        nint queue = 0;
        var createQueue = (delegate* unmanaged<nint, int*, Guid*, nint*, int>)vtable[CreateCommandQueueSlot];
        Assert.Equal(0, createQueue(device, queueDesc, &queueIid, &queue));

        var allocatorIid = new Guid("6102dee4-af59-4b09-b999-b44d73f09b24");
        nint allocator = 0;
        var createAllocator = (delegate* unmanaged<nint, int, Guid*, nint*, int>)vtable[CreateCommandAllocatorSlot];
        Assert.Equal(0, createAllocator(device, ComputeType, &allocatorIid, &allocator));

        int* listDesc = stackalloc int[] { ComputeType, 6, 1, 0 };
        nint list = 0;
        var createList = (delegate* unmanaged<nint, int*, nint, nint, Guid*, nint*, int>)vtable[CreateCommandListSlot];
        Assert.Equal(0, createList(device, listDesc, allocator, 0, &allocatorIid, &list));
        int closed = ((delegate* unmanaged<nint, int>)(*(nint**)list)[CloseSlot])(list);

        Assert.Equal(0, closed);
        Assert.NotEqual(0, queue);
        _ = ((delegate* unmanaged<nint, uint>)(*(nint**)queue)[ReleaseSlot])(queue);
    }

    [Fact]
    public void A_uav_texture_desc_is_one_the_desktop_accepts_and_keeps_the_flag_where_its_format_family_allows()
    {
        using var test = TestRuntime.Create();
        _ = new XboxDevice(test);
        nint desktop = test.Runtime.D3D12X.Objects.DesktopDevice;
        var descriptions = test.Runtime.D3D12X.Descriptions;

        for (int format = 1; format <= LastFormat; format++)
        {
            var plain = Texture(format, 0);
            var console = Texture(format, AllowUnorderedAccess);
            var converted = descriptions.ToDesktop(desktop, &console);

            // Formats the desktop refuses as textures at all say nothing about the flag.
            if (Size(desktop, plain) != ulong.MaxValue)
            {
                Assert.True(Size(desktop, converted) != ulong.MaxValue, $"format {format}, flags 0x{converted.Flags:x}");
            }
        }

        // A typeless texture keeps the flag through its UAV-capable members; the depth-stencil family has none.
        var rgba = Texture(Rgba8Typeless, AllowUnorderedAccess);
        var depth = Texture(DepthStencilTypeless, AllowUnorderedAccess);
        Assert.Equal(AllowUnorderedAccess, descriptions.ToDesktop(desktop, &rgba).Flags);
        Assert.Equal(0, descriptions.ToDesktop(desktop, &depth).Flags);
    }

    private static ResourceDescription Texture(int format, int flags)
    {
        return new ResourceDescription
        {
            Dimension = Texture2D, Width = TextureSize, Height = TextureSize, DepthOrArraySize = 1, MipLevels = 1, Format = format,
            SampleCount = 1, Flags = flags,
        };
    }

    private static ulong Size(nint device, ResourceDescription description)
    {
        ResourceAllocationInfo info;
        var query = (delegate* unmanaged<nint, ResourceAllocationInfo*, uint, uint, ResourceDescription*, ResourceAllocationInfo*>)(*(nint**)device)[AllocationInfoSlot];
        query(device, &info, 0, 1, &description);

        return info.Size;
    }
}
