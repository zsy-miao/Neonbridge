// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Tests.Cases;
using FiveStar.Shaders.Tests.Toolchain;
using FiveStar.Shaders.Tests.Warp;
using FiveStar.Shaders.Translation;
using Xunit;

namespace FiveStar.Shaders.Tests;

/// <summary>
/// The probe on a compute program (loader --probe-pipeline, FIVESTAR_STAGE_PROBE): the program still computes what it
/// did, and the first lane of each wave records the wave's site masks and its own loaded values. cs_buffer_bounds reads
/// a 32-byte raw buffer at 0x40 (past its end on every lane) and at 0x4 by s_buffer_load.
/// </summary>
public sealed class StageProbeTests : IClassFixture<WarpFixture>
{
    private const uint Id = 0x5EED;
    private const int RingSlot = 1;
    private const int DataSlot = 3;
    private const int Records = 64;
    private const uint Data = 0x3F800000;

    /// <summary>A pixel program in the translator's shape: one buffer load, returned as red.</summary>
    private const string PixelProgram = """
        #include "rdna_prelude.hlsli"
        struct Out {
          float4 mrt0 : SV_Target0;
        };
        Out main(float4 in_pos : SV_Position) {
          Out o = (Out)0;
          uint s0 = 0;
          s0 = select(buf_fits(0u, 16u), BufLoad(3u, 0u), 0u);
          o.mrt0 = float4(asfloat(s0), 0.0f, 0.0f, 1.0f);
          return o;
        }
        """;

    private readonly WarpFixture _fixture;

    public StageProbeTests(WarpFixture fixture)
    {
        _fixture = fixture;
    }

    [WarpFact]
    public void A_probed_compute_program_records_each_waves_failed_and_loaded_sites()
    {
        var (mismatches, records, sites) = new ComputeCaseRunner(_fixture).RunProbed(ComputeCases.Find("cs_buffer_bounds"), Id);
        var scalar = sites.Select((text, site) => (text, site)).Where(entry => entry.text.StartsWith("sbuf_fits", StringComparison.Ordinal)).Select(entry => entry.site).ToList();
        int past = scalar[0];
        int inside = scalar[1];

        Assert.True(mismatches.Count == 0, string.Join("\n", mismatches.Take(8)));
        Assert.NotEmpty(records);
        Assert.All(records, record => Assert.True(record.Whole && record.Stage && record.Id == Id && record.Sites == (uint)sites.Count));
        Assert.All(records, record => Assert.NotEqual(0u, record.RangeFailed & (1u << past)));
        Assert.All(records, record => Assert.Equal(0u, record.RangeFailed & (1u << inside)));

        // Every first lane loaded the same dword inside the buffer.
        Assert.Single(records.Select(record => record.Values[inside]).Distinct());
        Assert.NotNull(records[0].Values[inside]);
    }

    [WarpFact]
    public void A_probed_pixel_program_records_its_pixel_the_colour_it_returns_and_what_it_loaded()
    {
        var (probed, sites) = MeshProbe.Instrument(PixelProgram, Id, 0);
        var pixel = _fixture.Workbench.Compile("stage_probe_ps", probed, "ps");
        var vertex = _fixture.Helper("fullscreen_vs.hlsl", "vs");
        var bindings = new Bindings
        {
            Buffers =
            [
                new BufferResource(RingSlot, new uint[(MeshProbe.RecordsOffset / sizeof(uint)) + (Records * MeshProbeRecord.Dwords)], Writable: true),
                new BufferResource(DataSlot, [Data, 0, 0, 0], Writable: false),
            ],
        };

        var result = _fixture.Runner.Graphics(new GraphicsJob(vertex, pixel, Framebuffer.Fullscreen(1, PixelCase.Size, PixelCase.Size, true), bindings));
        var words = result.Buffers[RingSlot];
        var records = Enumerable.Range(0, (int)Math.Min(words[0], Records))
            .Select(index => MeshProbeRecord.Read(words.AsSpan((int)(MeshProbe.RecordsOffset / sizeof(uint)) + (index * MeshProbeRecord.Dwords), MeshProbeRecord.Dwords)))
            .ToList();

        Assert.Single(sites);
        Assert.NotEmpty(records);
        Assert.All(records, record =>
        {
            Assert.True(record.Whole && record.Stage && record.Id == Id);
            Assert.True(record.Group.X < PixelCase.Size && record.Group.Y < PixelCase.Size);
            Assert.Equal((1f, 0f, 0f, 1f), record.Position);
            Assert.Equal(Data, record.Values[0]);
            Assert.Equal(0u, record.RangeFailed);
        });
    }
}
