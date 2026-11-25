// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using FiveStar.Loader.Shims;
using FiveStar.Loader.Shims.D3D12X;
using Xunit;

namespace FiveStar.Loader.Tests;

/// <summary>Slot 74 is Wait64BitValueX, a wait the list records, and GPU write access follows the game's allocation flags.</summary>
public sealed unsafe class D3D12XMemoryWaitTests
{
    private const ulong Cell = 0x1475e75d0;
    private const uint GreaterEqual = 7;
    private const uint Flags = 1;

    [Fact]
    public void The_games_slot_74_call_records_a_full_64_bit_greater_or_equal_wait_and_writes_nothing()
    {
        var list = (XboxObject*)NativeMemory.AllocZeroed(XboxInterfaces.CommandListObjectSize);

        try
        {
            // 0x142484b0c: (address, 7, value, 1); the 1 is flags, not a mask.
            delegate* unmanaged<XboxObject*, ulong, uint, ulong, uint, int> wait64 = &MemoryWaits.Wait64;
            Assert.Equal(0, wait64(list, Cell, GreaterEqual, 0x1234_0000_0005, Flags));

            var waits = ListMemoryWaits.Of(list);
            Assert.Equal(1, waits->Count);
            Assert.Equal(Cell, waits->Address[0]);
            Assert.Equal(ulong.MaxValue, waits->Mask[0]);
            Assert.True(MemoryComparison.Holds(waits->Comparison[0], 0x1234_0000_0005, waits->Value[0], waits->Mask[0]));
            Assert.False(MemoryComparison.Holds(waits->Comparison[0], 0x0000_0000_0005, waits->Value[0], waits->Mask[0]));

            waits->Reset();
            Assert.Equal(0, waits->Count);
        }
        finally
        {
            NativeMemory.Free(list);
        }
    }

    [Fact]
    public void A_wait_after_the_lists_own_write_is_met_in_order_and_never_and_always_do_not_block()
    {
        var waits = (ListMemoryWaits*)NativeMemory.AllocZeroed((nuint)sizeof(ListMemoryWaits));

        try
        {
            waits->Wrote(Cell);
            Assert.True(waits->HasWritten(Cell));
            Assert.False(waits->HasWritten(Cell + 8));
            Assert.True(waits->Record(Cell + 8, sizeof(ulong), MemoryComparison.GreaterEqual, 1, ulong.MaxValue, 1));
            Assert.Equal(1, waits->Segment[0]);

            Assert.False(MemoryComparison.Blocks(MemoryComparison.Never));
            Assert.False(MemoryComparison.Blocks(MemoryComparison.Always));
            Assert.True(MemoryComparison.Holds(MemoryComparison.Equal, 0xFFFF_0001, 1, 0xFFFF));
        }
        finally
        {
            NativeMemory.Free(waits);
        }
    }

    [Theory]
    [InlineData(0x20003000u, 0x2404u, 2)]
    [InlineData(0x20003000u, 0x22004u, 2)]
    [InlineData(0x20406000u, 0x1004u, 1)]
    [InlineData(0x20406000u, 0x8404u, 1)]
    [InlineData(0x20403000u, 0x4u, 0)]
    [InlineData(GraphicsAllocations.MemGraphics | 0x3000u, 0x4u, 2)]
    public void Gpu_access_follows_the_allocation_flags_the_game_passed(uint type, uint protect, int expected)
    {
        Assert.Equal((GraphicsAccess)expected, GraphicsAllocations.Classify(type, protect));
    }
}
