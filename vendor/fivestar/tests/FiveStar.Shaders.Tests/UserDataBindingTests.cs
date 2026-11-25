// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Tests.Cases;
using FiveStar.Shaders.Tests.Toolchain;
using FiveStar.Shaders.Tests.Warp;
using Xunit;

namespace FiveStar.Shaders.Tests;

/// <summary>
/// The loader's two ways of passing user SGPRs, a root CBV over 128 dwords and 32 root constants
/// (--user-data-constants), give each stage the reference result: compute, pixel, vertex, geometry, and mesh drawn
/// natively and through the fallback (whose root descriptors share the root signature with the constants). All but the
/// geometry program read user SGPRs: the compute output descriptor, the pixel and vertex values, the mesh counts.
/// FIVESTAR_WARP_USER_DATA runs the whole suite either way.
/// </summary>
public sealed class UserDataBindingTests
{
    [WarpTheory]
    [InlineData(UserDataBinding.Buffer)]
    [InlineData(UserDataBinding.UserConstants)]
    public void Every_stage_reads_its_user_sgprs(UserDataBinding binding)
    {
        using var fixture = new WarpFixture(binding);
        var compute = new ComputeCaseRunner(fixture);
        var pixel = new PixelCaseRunner(fixture);
        var mesh = new MeshStageRunner(fixture);
        var runs = new Dictionary<string, IReadOnlyList<string>>
        {
            ["compute"] = compute.Run(ComputeCases.All().First()),
            ["compute, dxil backend"] = compute.RunDirect(ComputeCases.All().First()),
            ["pixel"] = pixel.Run(PixelCases.Find("cf_loop_scalar")),
            ["vertex"] = new VertexStageRunner(fixture).Run(),
            ["geometry"] = new GeometryStageRunner(fixture).Run(),
            ["mesh"] = mesh.Run(MeshStageRunner.Native),
            ["mesh fallback, table"] = mesh.Run(MeshStageRunner.FallbackTable),
            ["mesh fallback, root descriptors"] = mesh.Run(MeshStageRunner.FallbackRoot),
        };
        var failed = runs.Where(run => run.Value.Count != 0).Select(run => $"{run.Key}: {run.Value.Count} mismatches, {string.Join("; ", run.Value.Take(4))}");

        Assert.True(!failed.Any(), string.Join("\n", failed));
    }
}
