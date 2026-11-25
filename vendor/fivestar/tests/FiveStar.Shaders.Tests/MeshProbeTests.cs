// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Tests.Cases;
using FiveStar.Shaders.Tests.Toolchain;
using Xunit;

namespace FiveStar.Shaders.Tests;

/// <summary>
/// The mesh probe (loader --probe-pipeline) on the NGG program with four loads in front, in its two-phase and its
/// superstep form (what the wave32 variant runs): the group draws the same image, and its record holds the counts it
/// gave SetMeshOutputCounts and m0 they came from, its culled primitive, vertex 0's position, per load site whether its
/// range check failed, it read an unmapped page or a record naming no descriptor, and the value each site loaded.
/// </summary>
public sealed class MeshProbeTests : IClassFixture<WarpFixture>
{
    private const uint Id = 0x5EED;
    private const uint Allocation = (16 << 12) | 18;

    /// <summary>What the loads read: the pointer dword 0x108 / 4 of the page view, the good and null-record buffers' dword 0.</summary>
    private const uint PointerValue = 0xA000 + 66;
    private const uint DataValue = 0xB000;
    private const uint NullRecordValue = 0xC000;

    private readonly WarpFixture _fixture;

    public MeshProbeTests(WarpFixture fixture)
    {
        _fixture = fixture;
    }

    [WarpTheory]
    [InlineData(MeshStageRunner.Probe)]
    [InlineData(MeshStageRunner.ProbeSupersteps)]
    public void A_probed_mesh_group_records_its_counts_culling_position_and_loads(string variant)
    {
        var (mismatches, records, sites) = new MeshStageRunner(_fixture).RunProbed(variant, Id);
        int unmappedBuffer = Site(sites, "buf_fits", 0);
        int goodBuffer = Site(sites, "buf_fits", 1);
        int pointer = Site(sites, "PtrLoad", 0);
        int nullRecord = Site(sites, "buf_fits", 2);
        int window = Site(sites, "PtrLoad", 1);

        Assert.True(mismatches.Count == 0, string.Join("\n", mismatches.Take(8)));
        var record = Assert.Single(records);

        Assert.True(record.Whole);
        Assert.Equal((Id, (uint)sites.Count), (record.Id, record.Sites));
        Assert.Equal((18u, 16u, Allocation), (record.Vertices, record.Primitives, record.Allocation));
        Assert.Equal((1u, 1u << 5), (record.Culled, record.CullMask));
        Assert.Equal((-1f, 1f, 0f, 1f), record.Position);

        Assert.Equal(Bit(unmappedBuffer), record.RangeFailed);
        Assert.Equal(Bit(unmappedBuffer) | Bit(pointer), record.Unmapped);
        Assert.Equal(Bit(nullRecord), record.NullRecord);
        Assert.Equal(0u, (record.RangeFailed | record.Unmapped | record.NullRecord) & Bit(goodBuffer));

        // The failed check reads 0; the others what memory holds there.
        Assert.Equal<uint?>([0, DataValue, PointerValue, NullRecordValue], [record.Values[unmappedBuffer], record.Values[goodBuffer], record.Values[pointer], record.Values[nullRecord]]);

        // The 32-bit pointer reads the window, its address noted per site.
        Assert.Equal(Bit(window), record.WindowLoads);
        Assert.Equal<uint?>([MeshStageRunner.WindowAddress, MeshStageRunner.WindowAddress], [record.WindowAddress, record.WindowAddresses[window]]);
        Assert.Equal(MeshStageRunner.WindowFill + (MeshStageRunner.WindowAddress / 4), record.Values[window]);
        Assert.Null(record.WindowAddresses[pointer]);
    }

    /// <summary>The number of the <paramref name="nth"/> site whose text has <paramref name="call"/> (a buffer load's site is its check, buf_fits).</summary>
    private static int Site(IReadOnlyList<string> sites, string call, int nth)
    {
        var matching = sites.Select((text, index) => (text, index)).Where(site => site.text.Contains(call, StringComparison.Ordinal)).ToList();

        return matching[nth].index;
    }

    private static uint Bit(int site)
    {
        return 1u << Math.Min(site, 31);
    }
}
