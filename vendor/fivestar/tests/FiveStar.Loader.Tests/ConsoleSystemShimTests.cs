// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Tests.Support;
using Xunit;

namespace FiveStar.Loader.Tests;

/// <summary>The console system modules the game reaches during start-up: the APU heap and the DirectStorage factory.</summary>
public sealed unsafe class ConsoleSystemShimTests
{
    private const int Ok = 0;
    private const int NotImplemented = unchecked((int)0x80004001);
    private const uint HeapBytes = 0x100000;
    private const uint Alignment = 0x800;

    /// <summary>Slot 8 is the staging buffer size the game sets right after creating the factory; slot 3 is never called.</summary>
    private const int StagingBufferSizeSlot = 8;
    private const int UncalledSlot = 3;

    [Fact]
    public void Apu_heap_blocks_are_real_memory_and_translate_both_ways()
    {
        using var test = TestRuntime.Create();
        var create = (delegate* unmanaged<uint, uint, int>)test.Shim("AcpHal.dll", "ApuCreateHeap");
        var alloc = (delegate* unmanaged<nint*, uint*, uint, uint, uint, int>)test.Shim("AcpHal.dll", "ApuAlloc");
        var free = (delegate* unmanaged<nint, int>)test.Shim("AcpHal.dll", "ApuFree");
        var toApu = (delegate* unmanaged<nint, uint>)test.Shim("AcpHal.dll", "ApuMapVirtualAddress");
        var toVirtual = (delegate* unmanaged<uint, nint>)test.Shim("AcpHal.dll", "ApuMapApuAddress");

        nint first;
        nint second;
        nint reused;
        uint apu;

        Assert.Equal(Ok, create(HeapBytes, 0));
        Assert.Equal(Ok, alloc(&first, &apu, 100, Alignment, 0));
        Assert.Equal(Ok, alloc(&second, null, 100, Alignment, 0));
        *(long*)second = -1;

        Assert.Equal(0, first % Alignment);
        Assert.Equal(Alignment, (ulong)(second - first));
        Assert.Equal(apu, toApu(first));
        Assert.Equal(second, toVirtual(toApu(second)));

        Assert.Equal(Ok, free(first));
        Assert.Equal(Ok, alloc(&reused, null, 100, Alignment, 0));
        Assert.Equal(first, reused);
    }

    [Fact]
    public void Storage_factory_accepts_the_staging_size_and_refuses_a_slot_the_game_never_calls()
    {
        using var test = TestRuntime.Create();
        var getFactory = (delegate* unmanaged<Guid*, nint*, int>)test.Shim("XCRDAPI.dll", "DStorageGetFactory");
        var iid = Guid.Empty;
        nint factory;

        Assert.Equal(Ok, getFactory(&iid, &factory));
        nint* vtable = *(nint**)factory;
        var setStagingBufferSize = (delegate* unmanaged<nint, uint, int>)vtable[StagingBufferSizeSlot];
        var uncalled = (delegate* unmanaged<nint, nint, nint, nint, int>)vtable[UncalledSlot];

        Assert.Equal(Ok, setStagingBufferSize(factory, 0x2000000));
        Assert.Equal(NotImplemented, uncalled(factory, 0, 0, 0));
    }
}
