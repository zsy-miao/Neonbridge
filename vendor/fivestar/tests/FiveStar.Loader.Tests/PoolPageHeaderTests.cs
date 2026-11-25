// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Shims.D3D12X.DebugLayer;
using Xunit;

namespace FiveStar.Loader.Tests;

/// <summary>--watch-pool-pages tells the game's allocator's header changes from writes it cannot have made.</summary>
public sealed class PoolPageHeaderTests
{
    private const ulong Link = 0x2ff1a754000;
    private const ulong ListHead = 0x2ff3da1df08;

    /// <summary>A header: free offset, block count and counter packed as the allocator's compare-exchange writes them.</summary>
    private static PoolPageHeader Header(uint free, uint count, ushort counter, uint firstNext)
    {
        ulong freeList = free | ((ulong)count << 32) | ((ulong)counter << 48);

        return new PoolPageHeader(freeList, Link, ListHead, 0, firstNext);
    }

    [Fact]
    public void Allocations_and_frees_pass_and_garbage_does_not()
    {
        var before = Header(0x1230, 12, 41, 0x1240);

        // Three operations later: another offset, another count, the counter three on.
        Assert.True(before.CpuCanBecome(Header(0x0560, 9, 44, 0x0570)));
        Assert.True(before.CpuCanBecome(before));

        // The b9 failures: an offset past the page, a free block whose next offset is garbage, a counter thrown back.
        Assert.False(before.CpuCanBecome(Header(0x7FFFFFFF, 12, 42, 0)));
        Assert.False(before.CpuCanBecome(Header(0x1230, 12, 42, 0x80000001)));
        Assert.False(before.CpuCanBecome(Header(0x1230, 11, 40, 0x1240)));
        Assert.False(before.CpuCanBecome(new PoolPageHeader(ulong.MaxValue, ulong.MaxValue, ulong.MaxValue, 0xFF, uint.MaxValue)));
    }

    [Fact]
    public void A_given_back_page_and_the_crashs_current_page_are_told_apart()
    {
        var before = Header(0x1230, 12, 41, 0x1240);

        // Given back and reused: the counter does not move on.
        Assert.False(before.StillPooled(Header(0x0560, 549, 0, 0)));
        Assert.True(before.StillPooled(Header(0x7FFFFFFF, 12, 42, 0)));

        // The story-load crash's pool: current page 0x800000017fffffff.
        Assert.False(PoolObjectWatch.IsPage(0x8000_0001_7FFF_FFFF));
        Assert.True(PoolObjectWatch.IsPage(0x2250_28B0_000));
        Assert.True(PoolObjectWatch.IsPage(0));
    }

    [Fact]
    public void Zero_pages_and_unlinked_pages_are_not_taken_for_pool_pages()
    {
        Assert.False(new PoolPageHeader(0, 0, 0, 0, 0).Plausible());
        Assert.False(new PoolPageHeader(0x40, 0, 0, 0, 0).Plausible());
        Assert.True(Header(0x40, 1, 7, 0).Plausible());
    }
}
