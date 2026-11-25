// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Shims.D3D12X;
using FiveStar.Loader.Tests.Support;
using Xunit;

namespace FiveStar.Loader.Tests;

/// <summary>List slots 71 and 72 write their values to game memory when the list runs, and slot 74 holds the queue until memory holds its value, on the default adapter.</summary>
public sealed unsafe class D3D12XBottomOfPipeTests
{
    private const int CloseSlot = 9;
    private const int ExecuteSlot = 35;
    private const int SignalSlot = 16;
    private const int FenceValueSlot = 8;
    private const int FenceWaitSlot = 9;
    private const uint ReserveCommitLargePages = 0x20003000;
    private const ulong Flags = 0x283;
    private const uint GpuReadWrite = 0x2404;
    private const nuint Bytes = 0x20000;

    [Fact]
    public void A_list_writes_its_dword_and_qword_values_into_game_memory()
    {
        const ulong Counter = 0x1234_5678_9ABC_DEF0;
        const uint Marker = 0xC0FFEE;

        using var test = TestRuntime.Create();
        var virtualAlloc = (delegate* unmanaged<nint, nuint, uint, ulong, uint, nint>)test.Shim("xmem.dll", "XMemVirtualAlloc");
        byte* memory = (byte*)virtualAlloc(0, Bytes, ReserveCommitLargePages, Flags, GpuReadWrite);

        var device = new XboxDevice(test);
        nint queue = device.Queue();
        nint fence = device.Fence();
        nint list = device.DirectList();

        var write64 = (delegate* unmanaged<nint, ulong, ulong, ulong, uint, int>)XboxDevice.Vtable(list)[BottomOfPipeWrites.Write64Slot];
        var marker = (delegate* unmanaged<nint, ulong, uint, ulong, uint, int>)XboxDevice.Vtable(list)[BottomOfPipeWrites.MarkerSlot];
        Assert.Equal(0, write64(list, (ulong)(memory + 0x100), Counter, 0, 0));
        Assert.Equal(0, marker(list, (ulong)(memory + 0x200), Marker, 0, 0));

        Run(queue, list, fence, 1);
        Wait(fence, 1);

        Assert.Equal(Counter, *(ulong*)(memory + 0x100));
        Assert.Equal(Marker, *(uint*)(memory + 0x200));
    }

    [Fact]
    public void A_list_waiting_on_memory_runs_once_the_cpu_writes_the_value()
    {
        const ulong Frame = 0x677;
        const uint GreaterEqual = 7;

        using var test = TestRuntime.Create();
        var virtualAlloc = (delegate* unmanaged<nint, nuint, uint, ulong, uint, nint>)test.Shim("xmem.dll", "XMemVirtualAlloc");
        ulong* cell = (ulong*)virtualAlloc(0, Bytes, ReserveCommitLargePages, Flags, GpuReadWrite);

        var device = new XboxDevice(test);
        nint queue = device.Queue();
        nint fence = device.Fence();
        nint list = device.DirectList();

        // 0x142484b0c: Wait64BitValueX(address, GREATER_EQUAL, value, flags 1).
        var wait64 = (delegate* unmanaged<nint, ulong, uint, ulong, uint, int>)XboxDevice.Vtable(list)[MemoryWaits.Wait64Slot];
        Assert.Equal(0, wait64(list, (ulong)cell, GreaterEqual, Frame, 1));

        Run(queue, list, fence, 1);
        Thread.Sleep(200);
        Assert.Equal(0ul, ((delegate* unmanaged<nint, ulong>)XboxDevice.Vtable(fence)[FenceValueSlot])(fence));
        Assert.Equal(0ul, *cell);

        Volatile.Write(ref *cell, Frame);
        Wait(fence, 1);
    }

    [Fact]
    public void A_list_split_at_its_wait_runs_the_work_before_it_and_holds_only_the_work_after_it()
    {
        const ulong Before = 0x11;
        const ulong After = 0x22;
        const ulong Frame = 0x677;
        const uint GreaterEqual = 7;

        using var test = TestRuntime.Create();
        ListSplits.Enabled = true;
        var virtualAlloc = (delegate* unmanaged<nint, nuint, uint, ulong, uint, nint>)test.Shim("xmem.dll", "XMemVirtualAlloc");
        ulong* cells = (ulong*)virtualAlloc(0, Bytes, ReserveCommitLargePages, Flags, GpuReadWrite);

        var device = new XboxDevice(test);
        nint queue = device.Queue();
        nint fence = device.Fence();
        nint list = device.DirectList();
        var write64 = (delegate* unmanaged<nint, ulong, ulong, ulong, uint, int>)XboxDevice.Vtable(list)[BottomOfPipeWrites.Write64Slot];
        var wait64 = (delegate* unmanaged<nint, ulong, uint, ulong, uint, int>)XboxDevice.Vtable(list)[MemoryWaits.Wait64Slot];

        // A write, a wait on another cell, a write: the list is split at the wait.
        Assert.Equal(0, write64(list, (ulong)cells, Before, 0, 0));
        Assert.Equal(0, wait64(list, (ulong)(cells + 1), GreaterEqual, Frame, 1));
        Assert.Equal(0, write64(list, (ulong)(cells + 2), After, 0, 0));
        Assert.Equal(1, ListSegments.Of((XboxObject*)list)->Count);

        Run(queue, list, fence, 1);
        Thread.Sleep(200);
        Assert.Equal(Before, Volatile.Read(ref cells[0]));
        Assert.Equal(0ul, Volatile.Read(ref cells[2]));

        Volatile.Write(ref cells[1], Frame);
        Wait(fence, 1);
        Assert.Equal(After, cells[2]);
        ListSplits.Enabled = false;
    }

    private static void Run(nint queue, nint list, nint fence, ulong value)
    {
        Assert.Equal(0, ((delegate* unmanaged<nint, int>)XboxDevice.Vtable(list)[CloseSlot])(list));
        nint* lists = stackalloc nint[] { list };
        Assert.Equal(0, ((delegate* unmanaged<nint, uint, nint*, nint, uint, int>)XboxDevice.Vtable(queue)[ExecuteSlot])(queue, 1, lists, 0, 0));
        Assert.Equal(0, ((delegate* unmanaged<nint, nint, ulong, int>)XboxDevice.Vtable(queue)[SignalSlot])(queue, fence, value));
    }

    private static void Wait(nint fence, ulong value)
    {
        Assert.Equal(0, ((delegate* unmanaged<nint, ulong, nint, int>)XboxDevice.Vtable(fence)[FenceWaitSlot])(fence, value, 0));
    }
}
