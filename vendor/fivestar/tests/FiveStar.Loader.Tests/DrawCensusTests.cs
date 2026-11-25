// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Shims.D3D12X.DebugLayer;
using Xunit;

namespace FiveStar.Loader.Tests;

/// <summary>--census-passes: every pair a text names is listed, however few its draws, a skipped one with its stand-in's state.</summary>
public sealed class DrawCensusTests
{
    [Fact]
    public void The_census_lists_every_pass_a_text_names_with_its_stand_ins_state()
    {
        var output = new StringWriter();
        var census = new DrawCensus(new LoaderLog(output, null, false));
        var names = new Dictionary<nint, string> { [1] = "PS_GBuffer", [2] = "TL_PS_PBR_MATERIAL_STANDARD", [3] = "CS_Tonemap" };
        DrawCensus.Passes = ["TL_PS", "format 10"];

        try
        {
            for (int i = 0; i < 100; i++)
            {
                census.Add(1, 10, skipped: false);
            }

            census.Add(2, 20, skipped: true);
            census.Add(3, 20, skipped: false);
            census.Report(1, (pipeline, _) => names[pipeline], Describe, _ => "still building");
        }
        finally
        {
            DrawCensus.Passes = [];
        }

        string text = output.ToString();
        Assert.Contains("passes matching TL_PS, format 10: 2 pairs", text, StringComparison.Ordinal);
        Assert.Contains("TL_PS_PBR_MATERIAL_STANDARD -> 0x14 2560x1440 format 10: 1 skipped (stand-in still building)", text, StringComparison.Ordinal);
        Assert.Contains("CS_Tonemap -> 0x14 2560x1440 format 10: 1 recorded", text, StringComparison.Ordinal);
    }

    private static string Describe(nint target)
    {
        if (target == 20)
        {
            return "2560x1440 format 10";
        }

        return "2560x1440 format 28";
    }
}
