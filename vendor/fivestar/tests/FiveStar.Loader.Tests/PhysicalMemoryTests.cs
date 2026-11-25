// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Native;
using FiveStar.Loader.Shims.Storage;
using FiveStar.Loader.Tests.Support;
using Xunit;

namespace FiveStar.Loader.Tests;

/// <summary>xmem.dll physical pages behave like the console's: shared pages, real remaps, placeholders that come back.</summary>
public sealed unsafe class PhysicalMemoryTests : IDisposable
{
    private const ulong Page = 0x10000;

    /// <summary>MEM_RESERVE | MEM_64K_PAGES, and the XMem flags and protection the game passes with it.</summary>
    private const uint Reserve64K = 0x20402000;
    private const ulong GameFlags = 0x21;
    private const uint ReadWrite = 0x4;
    private const uint MemReserveState = 0x2000;

    private readonly TestRuntime _test = TestRuntime.Create();
    private readonly delegate* unmanaged<nint, nuint, uint, ulong, uint, nint> _virtualAlloc;
    private readonly delegate* unmanaged<ulong, ulong*, ulong*, int> _allocate;
    private readonly delegate* unmanaged<nint, ulong, ulong*, nint> _map;
    private readonly delegate* unmanaged<nint*, ulong, ulong*, int> _scatter;

    public PhysicalMemoryTests()
    {
        _virtualAlloc = (delegate* unmanaged<nint, nuint, uint, ulong, uint, nint>)_test.Shim("xmem.dll", "XMemVirtualAlloc");
        _allocate = (delegate* unmanaged<ulong, ulong*, ulong*, int>)_test.Shim("xmem.dll", "XMemAllocatePhysicalPages");
        _map = (delegate* unmanaged<nint, ulong, ulong*, nint>)_test.Shim("xmem.dll", "XMemMapPhysicalPages");
        _scatter = (delegate* unmanaged<nint*, ulong, ulong*, int>)_test.Shim("xmem.dll", "XMemMapPhysicalPagesScatter");
    }

    [Fact]
    public void A_page_mapped_at_two_addresses_is_one_page()
    {
        var pages = stackalloc ulong[2];
        AllocatePages(pages, 2);
        nint first = Reserve(2);
        nint second = Reserve(2);

        Assert.Equal(first, _map(first, 2, pages));
        Assert.Equal(second, _map(second, 2, pages));
        *(long*)(first + (nint)Page + 8) = 0x1234;

        Assert.Equal(0x1234, *(long*)(second + (nint)Page + 8));
        Assert.Equal(0, *(long*)(second + 8));
    }

    [Fact]
    public void A_page_keeps_its_contents_through_unmap_and_remap_and_the_range_maps_again()
    {
        var pages = stackalloc ulong[2];
        AllocatePages(pages, 2);
        nint range = Reserve(2);
        Assert.Equal(range, _map(range, 2, pages));
        *(long*)range = 7;
        *(long*)(range + (nint)Page) = 9;

        Assert.Equal(range, _map(range, 2, null));
        Assert.Equal(MemReserveState, StateOf(range));

        // Swapped: each page now sits where the other was, contents moving with it.
        var swapped = stackalloc ulong[] { pages[1], pages[0] };
        Assert.Equal(range, _map(range, 2, swapped));
        Assert.Equal(9, *(long*)range);
        Assert.Equal(7, *(long*)(range + (nint)Page));
    }

    [Fact]
    public void Scatter_maps_and_unmaps_single_pages()
    {
        var pages = stackalloc ulong[2];
        AllocatePages(pages, 2);
        nint range = Reserve(4);
        var addresses = stackalloc nint[] { range + (nint)(3 * Page), range + (nint)Page };

        Assert.Equal(1, _scatter(addresses, 2, pages));
        *(long*)addresses[0] = 5;
        *(long*)addresses[1] = 6;
        Assert.Equal(MemReserveState, StateOf(range));

        var unmapFirst = stackalloc ulong[] { 0, pages[1] };
        Assert.Equal(1, _scatter(addresses, 2, unmapFirst));
        Assert.Equal(MemReserveState, StateOf(addresses[0]));
        Assert.Equal(6, *(long*)addresses[1]);

        Assert.Equal(1, _scatter(addresses, 2, null));
        Assert.Equal(MemReserveState, StateOf(addresses[1]));
        Assert.Equal(1, _scatter(addresses, 1, pages));
        Assert.Equal(5, *(long*)addresses[0]);
    }

    [Fact]
    public void Pieces_of_a_game_split_placeholder_take_pages()
    {
        const uint ReservePlaceholder = 0x42000;
        const uint ReplacePlaceholder64K = 0x20406000;
        const uint SplitPlaceholder = 0x8002;
        const uint GpuReadWrite = 0x2004;

        var pages = stackalloc ulong[3];
        AllocatePages(pages, 3);
        nint whole = _virtualAlloc(0, (nuint)(3 * Page), ReservePlaceholder, 0x7, 0x1);
        Assert.NotEqual(0, whole);
        Assert.NotEqual(0, Kernel32.VirtualFree(whole, (nuint)Page, SplitPlaceholder));
        nint rest = whole + (nint)Page;

        Assert.Equal(whole, _virtualAlloc(whole, (nuint)Page, ReplacePlaceholder64K, 0x27, GpuReadWrite));
        Assert.Equal(rest, _virtualAlloc(rest, (nuint)(2 * Page), ReplacePlaceholder64K, 0x27, GpuReadWrite));
        Assert.Equal(whole, _map(whole, 1, pages));
        Assert.Equal(rest, _map(rest, 2, pages + 1));
        *(long*)(rest + (nint)Page) = 3;

        Assert.Equal(whole, _map(whole, 1, pages + 2));
        Assert.Equal(3, *(long*)whole);
    }

    [Fact]
    public void Pieces_mapped_out_of_order_from_one_allocation_lie_in_address_order_in_one_section()
    {
        const uint ReservePlaceholder = 0x42000;
        const uint ReplacePlaceholder64K = 0x20406000;
        const uint SplitPlaceholder = 0x8002;
        const uint GpuReadWrite = 0x2004;
        const int Pages = 4;

        // The GPU pool at 0x141E86400: one placeholder split in two pieces, mapped later piece first from one allocation.
        var pages = stackalloc ulong[Pages];
        AllocatePages(pages, Pages);
        nint whole = _virtualAlloc(0, (nuint)(Pages * Page), ReservePlaceholder, 0x7, 0x1);
        Assert.NotEqual(0, Kernel32.VirtualFree(whole, (nuint)(2 * Page), SplitPlaceholder));

        nint second = whole + (nint)(2 * Page);
        Assert.Equal(whole, _virtualAlloc(whole, (nuint)(2 * Page), ReplacePlaceholder64K, 0x27, GpuReadWrite));
        Assert.Equal(second, _virtualAlloc(second, (nuint)(2 * Page), ReplacePlaceholder64K, 0x27, GpuReadWrite));

        Assert.Equal(second, _map(second, 2, pages));
        Assert.Equal(whole, _map(whole, 2, pages + 2));

        var memory = _test.Runtime.XMemPhysical;
        Assert.True(memory.TryLocate(whole, out var chunk, out ulong offset));

        for (int page = 1; page < Pages; page++)
        {
            Assert.True(memory.TryLocate(whole + (nint)(page * (long)Page), out var next, out ulong nextOffset));
            Assert.Same(chunk, next);
            Assert.Equal(offset + ((ulong)page * Page), nextOffset);
        }
    }

    [Fact]
    public void A_page_list_storage_destination_writes_through_the_frames_into_every_mapping()
    {
        const ulong PageListOptions = 0x200001;
        const int OptionsWord = 0;
        const int DestinationWord = 1;
        const int FirstPageOffsetWord = 2;
        const int SizeWord = 3;
        const int Offset = 0xFFF0;

        // Mapped in reverse, so the write must follow the frame list, not the address order.
        var pages = stackalloc ulong[2];
        AllocatePages(pages, 2);
        var reversed = stackalloc ulong[] { pages[1], pages[0] };
        nint range = Reserve(2);
        Assert.Equal(range, _map(range, 2, reversed));

        RequestBlock block = default;
        Span<ulong> words = block;
        words[OptionsWord] = PageListOptions;
        words[DestinationWord] = (ulong)pages;
        words[FirstPageOffsetWord] = Offset;
        words[SizeWord] = 0x20;

        var data = new byte[0x20];
        data[0] = 1;
        data[0x10] = 2;

        var sink = new PhysicalPageSink(_test.Runtime.XMemPhysical, new UnsupportedDestinationSink(new LoaderLog(TextWriter.Null, null, false)));
        Assert.Equal(HResult.Ok, sink.Write(new StorageRequest(block), data));

        Assert.Equal(1, *(byte*)(range + (nint)Page + Offset));
        Assert.Equal(2, *(byte*)range);
    }

    [Fact]
    public void Copied_page_mappings_alias_the_pool_pages_and_unmap_the_rest()
    {
        var pages = stackalloc ulong[2];
        AllocatePages(pages, 2);
        nint pool = Reserve(2);
        Assert.Equal(pool, _map(pool, 2, pages));
        nint destination = Reserve(5);
        Assert.Equal(destination, _map(destination, 2, pages));

        // Pool pages 0 and 1 in order, one page unmapped, pool page 1 twice.
        var ranges = stackalloc Shims.PageRange[]
        {
            new() { Kind = Shims.PageRange.PoolPages, Start = 0, Count = 2 },
            new() { Kind = Shims.PageRange.Unmapped, Count = 1 },
            new() { Kind = Shims.PageRange.OnePoolPage, Start = 1, Count = 2 },
        };
        var batch = new Shims.PageBatch { Destination = (ulong)destination, Count = 3, Ranges = ranges };
        Assert.True(_test.Runtime.XMemPhysical.CopyPageMappings(1, &batch, pool, 2));

        *(long*)destination = 11;
        *(long*)(destination + (nint)(4 * Page)) = 12;

        Assert.Equal(11, *(long*)pool);
        Assert.Equal(12, *(long*)(pool + (nint)Page));
        Assert.Equal(12, *(long*)(destination + (nint)Page));
        Assert.Equal(12, *(long*)(destination + (nint)(3 * Page)));
        Assert.Equal(MemReserveState, StateOf(destination + (nint)(2 * Page)));
    }

    public void Dispose()
    {
        _test.Dispose();
    }

    private void AllocatePages(ulong* pages, ulong count)
    {
        ulong got = count;
        Assert.Equal(1, _allocate(0x280, &got, pages));
        Assert.Equal(count, got);
    }

    private nint Reserve(ulong pages)
    {
        nint address = _virtualAlloc(0, (nuint)(pages * Page), Reserve64K, GameFlags, ReadWrite);
        Assert.NotEqual(0, address);

        return address;
    }

    private static uint StateOf(nint address)
    {
        MemoryBasicInformation info;
        Assert.NotEqual(0u, (uint)Kernel32.VirtualQueryEx(Kernel32.GetCurrentProcess(), address, &info, (nuint)sizeof(MemoryBasicInformation)));

        return info.State;
    }
}
