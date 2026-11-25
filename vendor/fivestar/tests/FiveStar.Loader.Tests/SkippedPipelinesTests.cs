// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Shims.D3D12X.Pipelines;
using Xunit;

namespace FiveStar.Loader.Tests;

/// <summary>--skip-pipelines: which pipelines its texts name.</summary>
public sealed class SkippedPipelinesTests
{
    [Fact]
    public void A_pipeline_is_skipped_when_any_program_name_holds_one_of_the_texts()
    {
        var texts = SkippedPipelines.Parse(" LinearDepthSkin, ,CS_AdvectFromParentStep ");
        const string skin = "sgd1_0b7ec5e0_t32_PS_LinearDepthOpaque_f1f2249c_Wrapped + sgd1_04ab63d7_t41_MSNC_VS_LinearDepthSkin_f1f2249c_Wrapped";
        const string opaque = "sgd1_0b7ec5e0_t32_PS_LinearDepthOpaque_f1f2249c_Wrapped + sgd1_04a7a7b5_t41_MS_VS_TransformD_c5fd36bb_Wrapped";

        Assert.Equal(["LinearDepthSkin", "CS_AdvectFromParentStep"], texts);
        Assert.Equal("LinearDepthSkin", SkippedPipelines.Match(skin, texts));
        Assert.Equal("CS_AdvectFromParentStep", SkippedPipelines.Match("sgd1_0f8f1332_t3c_WindNew~CS_AdvectFromParentStep_54e861d9_Wrapped", texts));
        Assert.Null(SkippedPipelines.Match(opaque, texts));
        Assert.Null(SkippedPipelines.Match(skin, []));
        Assert.Null(SkippedPipelines.Match(skin, SkippedPipelines.Parse("lineardepthskin")));
    }

    [Fact]
    public void A_skipped_pipeline_is_not_waited_for()
    {
        const string skinned = "sgd1_0736b791_t32_PS_DeferredTextured_3030dd5f_Wrapped.trim2 + sgd1_01cb042f_t41_MSNC_VS_TransformSkinD_a1cf5e50_Wrapped.link-314dd11550b9";
        const string plain = "sgd1_07a9003b_t32_PS_DeferredTextured_c5fd36bb_Wrapped.trim2 + sgd1_02055286_t41_MS_VS_TransformD_c5fd36bb_Wrapped.link-377bf6b314e8";
        // Its own skip texts: the process's (SkippedPipelines.Texts) are set by every loader host other tests start.
        var waits = new PipelineWaits(["DeferredTextured"], PipelineWaits.DefaultLimit, SkippedPipelines.Parse("TransformSkinD"));

        Assert.False(waits.Matches(skinned));
        Assert.True(waits.Matches(plain));
    }
}
