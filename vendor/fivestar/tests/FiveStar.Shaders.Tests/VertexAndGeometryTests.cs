// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Tests.Cases;
using FiveStar.Shaders.Tests.Toolchain;
using Xunit;

namespace FiveStar.Shaders.Tests;

/// <summary>
/// A vertex shader with fetch-shader inputs, a handwritten legacy geometry shader with strips and cuts, and the real
/// GS_VoxelizeGeneric programs of the user's dump, each drawn on WARP and compared with a rasterizer model.
/// </summary>
public sealed class VertexAndGeometryTests : IClassFixture<WarpFixture>
{
    private readonly WarpFixture _fixture;

    public VertexAndGeometryTests(WarpFixture fixture)
    {
        _fixture = fixture;
    }

    [WarpFact]
    public void A_vertex_program_reads_its_fetch_shader_inputs()
    {
        var mismatches = new VertexStageRunner(_fixture).Run();

        Assert.True(mismatches.Count == 0, Report(mismatches));
    }

    [WarpFact]
    public void The_dxil_backend_compiles_the_vertex_program_with_its_fetch_inputs()
    {
        var mismatches = new VertexStageRunner(_fixture).Run(direct: true);

        Assert.True(mismatches.Count == 0, Report(mismatches));
    }

    [WarpFact]
    public void A_geometry_program_emits_strips_and_cuts()
    {
        var mismatches = new GeometryStageRunner(_fixture).Run();

        Assert.True(mismatches.Count == 0, Report(mismatches));
    }

    [WarpFact]
    public void The_dxil_backend_compiles_the_geometry_program_with_strips_and_cuts()
    {
        var mismatches = new GeometryStageRunner(_fixture).Run(direct: true);

        Assert.True(mismatches.Count == 0, Report(mismatches));
    }

    [RealProgramTheory]
    [InlineData("b300ca2e")]
    [InlineData("6be01b34")]
    public void The_dxil_backend_compiles_a_real_voxelize_program(string hash)
    {
        var mismatches = new RealGeometryRunner(_fixture).Run(RealProgramTheoryAttribute.Folder!, hash, direct: true);

        Assert.True(mismatches.Count == 0, Report(mismatches));
    }

    /// <summary>The two code variants of the program and one with a different output signature, by name hash.</summary>
    [RealProgramTheory]
    [InlineData("b300ca2e")]
    [InlineData("225d1bd4")]
    [InlineData("6be01b34")]
    public void A_real_voxelize_program_projects_each_triangle_on_its_dominant_axis(string hash)
    {
        var mismatches = new RealGeometryRunner(_fixture).Run(RealProgramTheoryAttribute.Folder!, hash);

        Assert.True(mismatches.Count == 0, Report(mismatches));
    }

    /// <summary>The executable's clear, which reads its vertices and colour through a root SRV address.</summary>
    [RealProgramTheory]
    [InlineData("VS_Clear_f05fef9c")]
    public void The_real_clear_program_reads_its_record_through_the_root_descriptor(string name)
    {
        var mismatches = new RealClearRunner(_fixture).Run(RealProgramTheoryAttribute.Folder!, name);

        Assert.True(mismatches.Count == 0, Report(mismatches));
    }

    private static string Report(IReadOnlyList<string> mismatches)
    {
        return $"{mismatches.Count} mismatches:\n{string.Join("\n", mismatches.Take(16))}";
    }
}
