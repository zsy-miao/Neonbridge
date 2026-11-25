// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using FiveStar.Loader.Shims.D3D12X;
using FiveStar.Loader.Tests.Support;
using Xunit;

namespace FiveStar.Loader.Tests;

/// <summary>The wrapped D3D12.X objects: queue, list, resource and fence slots, stubs, handlers and slot 133.</summary>
public sealed unsafe class D3D12XObjectTests
{
    private const uint Marker = 0x12345678;
    private const int NotImplemented = unchecked((int)0x80004001);
    private const int HandlerResult = 0x5A5A;

    [Fact]
    public void A_list_records_and_runs_through_the_xbox_queue_slots()
    {
        using var test = TestRuntime.Create();
        var device = new XboxDevice(test);
        nint upload = device.Buffer(XboxDevice.UploadHeap, 256, XboxDevice.GenericRead);
        nint readback = device.Buffer(XboxDevice.ReadbackHeap, 256, XboxDevice.CopyDest);
        XboxDevice.WriteFirst(upload, Marker);

        ulong completed = device.Copy(readback, upload, 0, sizeof(uint));

        Assert.Equal(1UL, completed);
        Assert.Equal(Marker, XboxDevice.ReadFirst(readback));
        Assert.NotEqual(upload, test.Runtime.D3D12X.Objects.Unwrap(upload));
    }

    [Fact]
    public void The_list_keeps_its_inline_recording_space_empty()
    {
        using var test = TestRuntime.Create();
        nint list = new XboxDevice(test).DirectList();

        // Write pointer +0x18, draw limit +0x28, limit +0x30: equal, so every inline command falls back to its slot.
        Assert.Equal(0, *(nint*)(list + 0x18));
        Assert.Equal(0, *(nint*)(list + 0x28));
        Assert.Equal(0, *(nint*)(list + 0x30));
    }

    [Fact]
    public void An_unknown_slot_logs_once_with_its_caller_and_a_handler_replaces_a_stub()
    {
        var log = new StringWriter();
        using var test = TestRuntime.Create(log: log);
        var device = new XboxDevice(test);
        nint list = device.DirectList();
        // Slot 130: no handler (131 and 132, the cache flushes, have one now).
        var unknown = (delegate* unmanaged<nint, int>)XboxDevice.Vtable(list)[130];

        Assert.Equal(NotImplemented, unknown(list));
        Assert.Equal(NotImplemented, unknown(list));
        delegate* unmanaged<nint, int> handler = &Handler;
        test.Runtime.D3D12X.Objects.SetHandler(XboxInterfaceKind.Device, 55, (nint)handler);
        int handled = ((delegate* unmanaged<nint, int>)XboxDevice.Vtable(device.Device)[55])(device.Device);

        string text = log.ToString();
        string name = "d3d12_xs.dll!ID3D12GraphicsCommandList.X::Slot130";
        Assert.Equal(2, test.Runtime.Stubs.CallsTo(name));
        Assert.Single(text.Split('\n'), line => line.Contains(name, StringComparison.Ordinal));
        Assert.Equal(HandlerResult, handled);
    }

    [Fact]
    public void Slot_133_places_a_buffer_on_the_game_s_own_pages()
    {
        using var test = TestRuntime.Create();
        var virtualAlloc = (delegate* unmanaged<nint, nuint, uint, ulong, uint, nint>)test.Shim("xmem.dll", "XMemVirtualAlloc");
        var allocate = (delegate* unmanaged<ulong, ulong*, ulong*, int>)test.Shim("xmem.dll", "XMemAllocatePhysicalPages");
        var map = (delegate* unmanaged<nint, ulong, ulong*, nint>)test.Shim("xmem.dll", "XMemMapPhysicalPages");

        ulong* pages = stackalloc ulong[2];
        ulong count = 2;
        Assert.Equal(1, allocate(0, &count, pages));
        nint address = virtualAlloc(0, 0x20000, 0x20402000, 0x21, 0x4);
        Assert.Equal(address, map(address, 2, pages));

        var device = new XboxDevice(test);
        Assert.Equal(0, device.PlacedBuffer(address, 0x20000, out nint buffer));
        *(uint*)(address + 0x10010) = Marker;
        nint readback = device.Buffer(XboxDevice.ReadbackHeap, 256, XboxDevice.CopyDest);
        device.Copy(readback, buffer, 0x10010, sizeof(uint));

        ulong gpuAddress = ((delegate* unmanaged<nint, ulong>)XboxDevice.Vtable(buffer)[11])(buffer);
        Assert.Equal((ulong)address, gpuAddress);
        Assert.True(test.Runtime.D3D12X.PlacedResources.Addresses.TryTranslate(gpuAddress + 0x10, out ulong desktop));
        Assert.NotEqual(0UL, desktop);
        Assert.Equal(Marker, XboxDevice.ReadFirst(readback));
    }

    [Fact]
    public void Fence_slot_12_hands_out_a_completed_value_cell_that_follows_the_fence()
    {
        const int FenceSignalSlot = 10;
        const int CompletedValueSlot = 12;
        const ulong Value = 7;
        using var test = TestRuntime.Create();
        nint fence = new XboxDevice(test).Fence();

        ulong* cell = ((delegate* unmanaged<nint, ulong*>)XboxDevice.Vtable(fence)[CompletedValueSlot])(fence);
        Assert.Equal(0UL, *cell);
        Assert.Equal(0, ((delegate* unmanaged<nint, ulong, int>)XboxDevice.Vtable(fence)[FenceSignalSlot])(fence, Value));

        Assert.True(SpinWait.SpinUntil(() => Volatile.Read(ref *cell) == Value, TimeSpan.FromSeconds(5)));
    }

    [Fact]
    public void A_gpu_address_in_memory_the_game_committed_translates_to_a_desktop_buffer()
    {
        const uint ReserveCommitLargePages = 0x20003000;
        const uint GpuReadWrite = 0x2404;
        const ulong Offset = 0x10010;

        using var test = TestRuntime.Create();
        var virtualAlloc = (delegate* unmanaged<nint, nuint, uint, ulong, uint, nint>)test.Shim("xmem.dll", "XMemVirtualAlloc");
        nint address = virtualAlloc(0, 0x20000, ReserveCommitLargePages, 0x283, GpuReadWrite);
        _ = new XboxDevice(test);
        var memory = test.Runtime.D3D12X.GameMemory;

        Assert.True(memory.TryTranslate((ulong)address, out ulong start));
        Assert.True(memory.TryTranslate((ulong)address + Offset, out ulong inside));
        Assert.Equal(start + Offset, inside);
    }

    [UnmanagedCallersOnly]
    private static int Handler(nint self)
    {
        return HandlerResult;
    }
}
