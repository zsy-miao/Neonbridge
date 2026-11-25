// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Translation;
using Xunit;

namespace FiveStar.Shaders.Tests;

/// <summary>The static samplers' slots and header lines (StaticSampler).</summary>
public sealed class StaticSamplerTests
{
    // Every literal S# of the 25,908-program corpus (6,011 programs use one), with whether a compare uses it.
    private static readonly (uint W0, uint W2, bool Compare)[] Corpus =
    [
        (0x92, 0x18500000, false), (0x0, 0x18500000, false), (0x92, 0x14500000, false), (0x1b6, 0x18500000, false),
        (0x3092, 0x18500000, true), (0x0, 0x14500000, false), (0x400, 0x18f00000, false), (0x2, 0x18500000, false),
        (0x180, 0x14500000, false), (0x5b6, 0x18f00000, false), (0x200, 0x18f03e80, false), (0x492, 0x18f00000, false),
        (0x692, 0x18f00000, false), (0x1b6, 0x14500000, false), (0x0, 0x18503f80, false), (0x32, 0x18500000, false),
        (0x12, 0x18500000, false), (0x36, 0x18500000, false),
    ];

    [Fact]
    public void Every_literal_sampler_of_the_corpus_has_a_slot_of_its_own()
    {
        var slots = Corpus.Select(sampler => StaticSampler.SlotOf([sampler.W0, 0xc00000, sampler.W2, 0], sampler.Compare)).ToList();

        Assert.Equal(slots.Count, slots.Distinct().Count());
        Assert.All(slots, slot => Assert.InRange(slot, StaticSampler.FirstSlot, StaticSampler.FirstSlot + StaticSampler.Slots - 1));
    }

    [Fact]
    public void A_header_line_reads_back_with_its_slot_and_use()
    {
        var sampler = StaticSampler.Of([0x3092, 0xc00000, 0x18500000, 0], comparison: true);
        var read = Assert.Single(StaticSampler.Read("#include \"x\"\n" + sampler.Line + "\n"));

        Assert.True(read.SameAs(sampler));
        Assert.EndsWith(" compare", sampler.Line, StringComparison.Ordinal);
    }
}
