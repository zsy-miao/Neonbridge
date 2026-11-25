// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Shims.D3D12X;
using Xunit;

namespace FiveStar.Loader.Tests.Support;

/// <summary>
/// A D3D12.X device made through the shim, called the way the game calls it: raw vtable slots in D3D12.X numbering.
/// Every object it returns is the shim's wrapper.
/// </summary>
internal sealed unsafe class XboxDevice
{
    public const int DirectType = 0;
    public const int ComputeType = 2;
    public const int UploadHeap = 2;
    public const int ReadbackHeap = 3;
    public const int GenericRead = 0xAC3;
    public const int CopyDest = 0x400;

    private static readonly Guid QueueIid = new("0ec870a6-5d7e-4c22-8cfc-5baae07616ed");
    private static readonly Guid AllocatorIid = new("6102dee4-af59-4b09-b999-b44d73f09b24");
    private static readonly Guid ListIid = new("c3827890-4024-474c-98cf-645b51dc4874");
    private static readonly Guid ResourceIid = new("696442be-a72e-4059-bc79-5b5c98040fad");
    private static readonly Guid FenceIid = new("0a753dcf-c4d8-4b91-adf6-be5a60d95a76");

    public XboxDevice(TestRuntime test)
    {
        var create = (delegate* unmanaged<nint, byte*, Guid*, nint*, int>)test.Shim("d3d12_xs.dll", "D3D12XboxCreateDevice");
        byte* parameters = stackalloc byte[0x50];
        var iid = Guid.Empty;
        nint device = 0;
        Assert.Equal(0, create(0, parameters, &iid, &device));
        Device = device;
    }

    public nint Device { get; }

    public static nint* Vtable(nint obj)
    {
        return *(nint**)obj;
    }

    /// <summary>Device slot 8, a direct queue.</summary>
    public nint Queue()
    {
        int* desc = stackalloc int[] { DirectType, 0, 0, 0 };
        Guid iid = QueueIid;
        nint queue = 0;
        Assert.Equal(0, ((delegate* unmanaged<nint, int*, Guid*, nint*, int>)Vtable(Device)[8])(Device, desc, &iid, &queue));

        return queue;
    }

    /// <summary>Device slot 9 for the allocator, then X slot 73 with the game's desc and list IID.</summary>
    public nint DirectList()
    {
        return List(DirectType);
    }

    /// <summary>A compute list, made as <see cref="DirectList"/>.</summary>
    public nint ComputeList()
    {
        return List(ComputeType);
    }

    /// <summary>Device slot 9: a command allocator of <paramref name="type"/>.</summary>
    public nint Allocator(int type)
    {
        Guid allocatorIid = AllocatorIid;
        nint allocator = 0;
        Assert.Equal(0, ((delegate* unmanaged<nint, int, Guid*, nint*, int>)Vtable(Device)[9])(Device, type, &allocatorIid, &allocator));

        return allocator;
    }

    private nint List(int type)
    {
        nint allocator = Allocator(type);
        int* desc = stackalloc int[] { type, 6, 1, 0 };
        Guid listIid = ListIid;
        nint list = 0;
        var create = (delegate* unmanaged<nint, int*, nint, nint, Guid*, nint*, int>)Vtable(Device)[73];
        Assert.Equal(0, create(Device, desc, allocator, 0, &listIid, &list));

        return list;
    }

    /// <summary>Device slot 27: a committed buffer on an upload or readback heap.</summary>
    public nint Buffer(int heapType, ulong size, int state)
    {
        int* properties = stackalloc int[] { heapType, 0, 0, 0, 0 };
        var desc = BufferDescription(size);
        Guid iid = ResourceIid;
        nint resource = 0;
        var create = (delegate* unmanaged<nint, int*, int, ResourceDescription*, int, void*, Guid*, nint*, int>)Vtable(Device)[27];
        Assert.Equal(0, create(Device, properties, 0, &desc, state, null, &iid, &resource));

        return resource;
    }

    /// <summary>X slot 133: a buffer at a game address.</summary>
    public int PlacedBuffer(nint address, ulong size, out nint resource)
    {
        var desc = BufferDescription(size);
        Guid iid = ResourceIid;
        nint created = 0;
        var create = (delegate* unmanaged<nint, nint, ResourceDescription*, uint, void*, Guid*, nint*, int>)Vtable(Device)[133];
        int hr = create(Device, address, &desc, uint.MaxValue, null, &iid, &created);
        resource = created;

        return hr;
    }

    /// <summary>Device slot 36.</summary>
    public nint Fence()
    {
        Guid iid = FenceIid;
        nint fence = 0;
        Assert.Equal(0, ((delegate* unmanaged<nint, ulong, int, Guid*, nint*, int>)Vtable(Device)[36])(Device, 0, 0, &iid, &fence));

        return fence;
    }

    /// <summary>
    /// Copies <paramref name="size"/> bytes on the GPU through the D3D12.X slots: list 15 (CopyBufferRegion) and 9
    /// (Close), queue 35 (ExecuteCommandListsX) and 16 (Signal), then fence 9 (SetEventOnCompletion with no event
    /// blocks until the GPU is done). Returns the fence's completed value (fence slot 8).
    /// </summary>
    public ulong Copy(nint destination, nint source, ulong sourceOffset, ulong size)
    {
        nint queue = Queue();
        nint list = DirectList();
        nint fence = Fence();
        ((delegate* unmanaged<nint, nint, ulong, nint, ulong, ulong, void>)Vtable(list)[15])(list, destination, 0, source, sourceOffset, size);
        Assert.Equal(0, ((delegate* unmanaged<nint, int>)Vtable(list)[9])(list));

        nint* lists = stackalloc nint[] { list };
        Assert.Equal(0, ((delegate* unmanaged<nint, uint, nint*, nint, uint, int>)Vtable(queue)[35])(queue, 1, lists, 0, 3));
        Assert.Equal(0, ((delegate* unmanaged<nint, nint, ulong, int>)Vtable(queue)[16])(queue, fence, 1));
        Assert.Equal(0, ((delegate* unmanaged<nint, ulong, nint, int>)Vtable(fence)[9])(fence, 1, 0));

        return ((delegate* unmanaged<nint, ulong>)Vtable(fence)[8])(fence);
    }

    /// <summary>Runs a closed <paramref name="list"/> on a new direct queue (X slot 35) and blocks until the GPU is done.</summary>
    public void Run(nint list)
    {
        nint queue = Queue();
        nint fence = Fence();
        nint* lists = stackalloc nint[] { list };
        Assert.Equal(0, ((delegate* unmanaged<nint, uint, nint*, nint, uint, int>)Vtable(queue)[35])(queue, 1, lists, 0, 3));
        Assert.Equal(0, ((delegate* unmanaged<nint, nint, ulong, int>)Vtable(queue)[16])(queue, fence, 1));
        Assert.Equal(0, ((delegate* unmanaged<nint, ulong, nint, int>)Vtable(fence)[9])(fence, 1, 0));
    }

    /// <summary>Resource slots 8 and 9 (Map, Unmap): the first dword of a CPU-visible buffer.</summary>
    public static uint ReadFirst(nint resource)
    {
        void* data = null;
        Assert.Equal(0, ((delegate* unmanaged<nint, uint, void*, void**, int>)Vtable(resource)[8])(resource, 0, null, &data));
        uint value = *(uint*)data;
        ((delegate* unmanaged<nint, uint, void*, void>)Vtable(resource)[9])(resource, 0, null);

        return value;
    }

    public static void WriteFirst(nint resource, uint value)
    {
        void* data = null;
        Assert.Equal(0, ((delegate* unmanaged<nint, uint, void*, void**, int>)Vtable(resource)[8])(resource, 0, null, &data));
        *(uint*)data = value;
        ((delegate* unmanaged<nint, uint, void*, void>)Vtable(resource)[9])(resource, 0, null);
    }

    private static ResourceDescription BufferDescription(ulong size)
    {
        return new ResourceDescription
        {
            Dimension = ResourceDescription.BufferDimension,
            Width = size,
            Height = 1,
            DepthOrArraySize = 1,
            MipLevels = 1,
            SampleCount = 1,
            Layout = 1,
        };
    }
}
