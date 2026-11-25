// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using FiveStar.Loader.Shims.D3D12X.Binding;
using Xunit;

namespace FiveStar.Loader.Tests;

/// <summary>
/// The cap table after the window guard's: programs report threads past the block cap, groups past the round bound and
/// waves past the loop cap per program slot, and each report names only what grew since the last.
/// </summary>
public sealed unsafe class SuperstepCapTableTests
{
    private const int Slots = 1024;
    private const int SlotDwords = 4;

    [Fact]
    public void Each_report_names_the_programs_whose_cap_counts_grew_since_the_last()
    {
        uint* table = (uint*)NativeMemory.AllocZeroed(Slots * SlotDwords * sizeof(uint));
        var seen = new CapCounts(new uint[Slots], new uint[Slots], new uint[Slots]);

        try
        {
            // LinearDepthSkin's id in its slot: 3,456 threads ran out of blocks; another program ran out of rounds twice,
            // a third has 5 waves past the loop cap.
            Write(table, 0x5EED, 3456, 0, 0);
            Write(table, 0x0123, 0, 2, 0);
            Write(table, 0x0777, 0, 0, 5);

            Assert.Equal([new CapHit(0x0123, 0, 2, 0), new CapHit(0x5EED, 3456, 0, 0), new CapHit(0x0777, 0, 0, 5)], WindowGuard.CapHits(table, seen));

            // Nothing new: no report. Then 128 more threads of the first.
            Assert.Empty(WindowGuard.CapHits(table, seen));
            Write(table, 0x5EED, 3456 + 128, 0, 0);
            Assert.Equal([new CapHit(0x5EED, 128, 0, 0)], WindowGuard.CapHits(table, seen));

            Assert.Contains($"#define {WindowGuard.CapDefine} {WindowGuard.Base + WindowGuard.CapOffset}u", WindowGuard.Header, StringComparison.Ordinal);
            Assert.True(WindowGuard.CapOffset + (Slots * SlotDwords * sizeof(uint)) <= WindowGuard.RegionBytes);
        }
        finally
        {
            NativeMemory.Free(table);
        }
    }

    [Fact]
    public void The_sampler_and_the_presenter_report_a_cap_hit_once_between_them()
    {
        uint* table = (uint*)NativeMemory.AllocZeroed(Slots * SlotDwords * sizeof(uint));
        using var text = new StringWriter();
        var log = new FiveStar.Loader.Diagnostics.LoaderLog(TextWriter.Null, TextWriter.Synchronized(text), false);

        try
        {
            Write(table, 0x2BAD, 777, 0, 0);
            Parallel.For(0, 8, _ => WindowGuard.ReportCaps(table, 2300, log));

            string lines = text.ToString();
            Assert.Single(lines.Split('\n'), line => line.Contains("program 0x00002bad: 777 threads", StringComparison.Ordinal));
        }
        finally
        {
            NativeMemory.Free(table);
        }
    }

    /// <summary>The slot of <paramref name="id"/> (id &amp; 1023) as a program leaves it.</summary>
    private static void Write(uint* table, uint id, uint blocks, uint rounds, uint loops)
    {
        uint* slot = table + ((id & (Slots - 1)) * SlotDwords);
        (slot[0], slot[1], slot[2], slot[3]) = (id, blocks, rounds, loops);
    }
}
