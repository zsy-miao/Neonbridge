// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Native;
using FiveStar.Loader.Tests.Support;
using Xunit;

namespace FiveStar.Loader.Tests;

/// <summary>The memory map's heap segments, each given to its heap and the heap to whoever made it.</summary>
public sealed unsafe class HeapOwnersTests
{
    private const uint NtSegmentSignature = 0xFFEEFFEE;
    private const nuint Block = 256 * 1024;
    private const int Blocks = 256;

    [Fact]
    public void A_grown_heap_owns_all_its_segments_and_is_named_by_how_it_was_made()
    {
        using var test = TestRuntime.Create();
        var create = (delegate* unmanaged<uint, nuint, nuint, nint>)test.Shim("kernel32.dll", "HeapCreate");
        var destroy = (delegate* unmanaged<nint, int>)test.Shim("kernel32.dll", "HeapDestroy");
        nint heap = create(0, 0, 0);

        try
        {
            // 64 MB in blocks below the large-block size: the heap adds segments as it grows.
            for (int i = 0; i < Blocks; i++)
            {
                Assert.NotEqual(0, HeapAlloc(heap, 0, Block));
            }

            var walk = MemoryRegionWalk.Run();
            var heaps = new HashSet<nint>(HeapUsage.Handles(1024));
            var segments = walk.Allocations.Values.Where(a => a.Signature == NtSegmentSignature || heaps.Contains(a.Base)).Select(a => (Owner: HeapOwners.Owner(a.Base, a.Signature, heaps), Bytes: a.Private)).ToList();
            string summary = HeapOwners.Summary(segments, test.Runtime.Images, 64);

            Assert.True(segments.Count(segment => segment.Owner == heap) > 1, summary);
            Assert.Contains($"0x{heap:x} (made through the shims outside the game's images)", summary, StringComparison.Ordinal);
            Assert.StartsWith("the process heap", HeapOwners.Describe(Kernel32.GetProcessHeap(), test.Runtime.Images), StringComparison.Ordinal);
        }
        finally
        {
            Assert.NotEqual(0, destroy(heap));
        }
    }

    [System.Runtime.InteropServices.DllImport("kernel32.dll")]
    private static extern nint HeapAlloc(nint heap, uint flags, nuint bytes);
}
