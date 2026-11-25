// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Shims.D3D12X.Binding;
using Xunit;

namespace FiveStar.Loader.Tests;

/// <summary>--hold-passes and --stub-passes: what a named pass's recorded work does, frame by frame.</summary>
public sealed class PassRulesTests
{
    private const string Apply = "sgd1_0d3fa3c8_t32_scattering_transmittance_system~CS_ApplyCombinedDeferred_1e5750b4_Wrapped";
    private const string Reconstruct = "sgd1_0d3e1f3b_t32_scattering_transmittance_system~CS_ReconstructRaymarchClassifiedFull_2d384eab_Wrapped";
    private const string Sort = "sgd1_x_t32_scattering_transmittance_system~CS_SortRayTiles_y_Wrapped";

    [Fact]
    public void A_consumer_is_held_while_a_producer_has_not_run_or_was_skipped_this_frame()
    {
        var rules = PassRules.Parse("ApplyCombinedDeferred=ReconstructRaymarch|SortRayTiles", null);

        // No producer has run yet.
        Assert.Equal(PassAction.Hold, rules.Decide(Apply, out _, out var reason));
        Assert.Contains("ReconstructRaymarch", reason, StringComparison.Ordinal);

        rules.Note(Reconstruct, skipped: false);
        rules.Note(Sort, skipped: false);
        Assert.Equal(PassAction.Run, rules.Decide(Apply, out _, out _));

        // A frame where a producer is a stand-in again.
        rules.NextFrame();
        rules.Note(Reconstruct, skipped: true);
        Assert.Equal(PassAction.Hold, rules.Decide(Apply, out _, out _));

        // The next frame it ran; other pipelines are never held.
        rules.NextFrame();
        Assert.Equal(PassAction.Run, rules.Decide(Apply, out _, out _));
        Assert.Equal(PassAction.Run, rules.Decide(Reconstruct, out _, out _));
    }

    [Fact]
    public void A_stubbed_pass_takes_its_colour_and_a_malformed_rule_is_refused()
    {
        var rules = PassRules.Parse(null, "PS_ShadowMask=1,1,1,1;CS_Exposure=0.5, 0.5, 0.5, 1");

        Assert.True(rules.Enabled);
        Assert.Equal(PassAction.Stub, rules.Decide("sgd1_a_t32_PS_ShadowMask_b_Wrapped + sgd1_c_t32_VS_FullScreen_d_Wrapped", out var colour, out _));
        Assert.Equal([1f, 1f, 1f, 1f], colour!);
        Assert.Equal(PassAction.Stub, rules.Decide("x~CS_Exposure_y", out colour, out _));
        Assert.Equal([0.5f, 0.5f, 0.5f, 1f], colour!);

        Assert.False(PassRules.Parse(null, null).Enabled);
        Assert.Throws<ArgumentException>(() => PassRules.Parse(null, "PS_ShadowMask=1,1,1"));
        Assert.Throws<ArgumentException>(() => PassRules.Parse("ApplyCombinedDeferred", null));
    }
}
