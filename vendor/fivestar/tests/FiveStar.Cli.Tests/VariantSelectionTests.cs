// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Graphics;
using FiveStar.Graphics.Programs;
using Xunit;

namespace FiveStar.Cli.Tests;

/// <summary>Each adapter runs the form of a program that gives console results, or the program is reported.</summary>
public sealed class VariantSelectionTests
{
    private static readonly AdapterInfo Nvidia = new(0, "wave32 with mesh shaders", false, "6.8", 32, 32, "Tier1");
    private static readonly AdapterInfo Amd = new(1, "wave32-64 without mesh shaders", false, "6.8", 32, 64, "NotSupported");

    private static readonly ProgramVariants PinnedCompute = new("cs", "cs", "cs.dxil", null, null, 64, "cs.w32.dxil", null);
    private static readonly ProgramVariants Mesh = new("ms", "ms", "ms.dxil", 64, 32, null, null, new MeshFallbackFiles("ms.msfb_cs.dxil", "ms.msfb_vs.dxil"));
    private static readonly ProgramVariants WidePixel = new("ps", "ps", "ps.dxil", 64, 64, null, null, null);
    private static readonly ProgramVariants WideMesh = new("wm", "ms", "wm.dxil", 64, 64, null, "wm.w32.dxil", null);

    // Over the mesh output limit, floor 64: the fallback pinned to 64 lanes, or the fallback of its superstep form.
    private static readonly ProgramVariants WideFallback = new("wf", "ms", null, 64, 64, null, null, new MeshFallbackFiles("wf.msfb_cs.dxil", "wf.msfb_vs.dxil")
    {
        PinnedCompute = "wf.msfb_cs.w64.dxil", PinnedLanes = 64, Supersteps = new MeshFallbackFiles("wf.w32.msfb_cs.dxil", "wf.w32.msfb_vs.dxil"),
    });

    [Fact]
    public void Wave_pins_mesh_support_and_floors_pick_the_form()
    {
        Assert.Equal((ChoiceOutcome.Variant, "cs.w32.dxil"), Pick(Nvidia, PinnedCompute));
        Assert.Equal((ChoiceOutcome.Native, "cs.dxil"), Pick(Amd, PinnedCompute));

        Assert.Equal((ChoiceOutcome.Native, "ms.dxil"), Pick(Nvidia, Mesh));
        Assert.Equal((ChoiceOutcome.Fallback, "ms.msfb_cs.dxil"), Pick(Amd, Mesh));

        // The AMD driver may run a pixel shader on 32-lane waves, below the floor of 64.
        Assert.Equal(ChoiceOutcome.Incorrect, VariantSelector.Choose(Nvidia, WidePixel).Outcome);
        Assert.Equal(ChoiceOutcome.Incorrect, VariantSelector.Choose(Amd, WidePixel).Outcome);

        // A mesh program that needs its whole console wave runs its superstep form on 32-lane waves, also from a
        // translation for 32-lane GPUs only, which did not compile the native form.
        Assert.Equal((ChoiceOutcome.Variant, "wm.w32.dxil"), Pick(Nvidia, WideMesh));
        Assert.Equal((ChoiceOutcome.Variant, "wm.w32.dxil"), Pick(Nvidia, WideMesh with { Dxil = null }));

        // An adapter whose probe ran mesh shaders in 64-lane waves (an RX 7800 XT, 32-64) runs the native form; one that
        // was not measured or ran them in 32 keeps the superstep form. The measurement is for mesh shaders only.
        var rdna3 = new AdapterInfo(2, "wave32-64, mesh shaders in 64-lane waves", false, "6.8", 32, 64, "Tier1") { MeshWaveLanes = 64 };
        Assert.Equal((ChoiceOutcome.Native, "wm.dxil"), Pick(rdna3, WideMesh));
        Assert.Equal((ChoiceOutcome.Variant, "wm.w32.dxil"), Pick(rdna3 with { MeshWaveLanes = null }, WideMesh));
        Assert.Equal((ChoiceOutcome.Variant, "wm.w32.dxil"), Pick(rdna3 with { MeshWaveLanes = 32 }, WideMesh));
        Assert.Equal(ChoiceOutcome.Incorrect, VariantSelector.Choose(rdna3, WidePixel).Outcome);

        // A fallback below its floor runs pinned where 64-lane waves exist, else in supersteps.
        Assert.Equal((ChoiceOutcome.Fallback, "wf.msfb_cs.w64.dxil"), Pick(Amd, WideFallback));
        Assert.Equal((ChoiceOutcome.Fallback, "wf.w32.msfb_cs.dxil"), Pick(Nvidia, WideFallback));
        Assert.Equal((ChoiceOutcome.Fallback, "wf.w32.msfb_cs.dxil"), Pick(AdapterProfiles.IntelXe2, WideFallback));
    }

    private static (ChoiceOutcome Outcome, string File) Pick(AdapterInfo adapter, ProgramVariants program)
    {
        var choice = VariantSelector.Choose(adapter, program);

        return (choice.Outcome, choice.Files[0]);
    }
}
