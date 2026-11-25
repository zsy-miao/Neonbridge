// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Shims.D3D12X.Pipelines;
using Xunit;

namespace FiveStar.Loader.Tests;

/// <summary>
/// Without --raytracing a pipeline whose programs trace rays stays a stand-in: known when its program is translated,
/// and in a later boot or from the compile host by the marker beside its DXIL; other pipelines are built.
/// </summary>
public sealed class RayTracingProgramsTests : IDisposable
{
    private const string Tracing = "sgd1_0e14c743_t41_standard_doublesided~CS_RaytracingResolve_b1139868_Wrapped";
    private const string Earlier = "sgd1_0f2cdead_t32_rtdirectionalshadows~CS_TraceShadows_12345678_Wrapped";
    private const string Plain = "sgd1_0b7ec5e0_t32_PS_LinearDepthOpaque_f1f2249c_Wrapped";

    private readonly string _folder = Directory.CreateTempSubdirectory("fivestar-rt-").FullName;

    [Fact]
    public void A_pipeline_whose_programs_trace_rays_stays_a_stand_in()
    {
        RayTracingPrograms.Note(Tracing, "#include \"rdna_prelude.hlsli\"\n#include \"rdna_bvh.hlsli\"\nvoid main() {}", _folder);
        RayTracingPrograms.Note(Plain, "#include \"rdna_prelude.hlsli\"\nvoid main() {}", _folder);

        Assert.True(File.Exists(Path.Combine(_folder, Tracing + RayTracingPrograms.MarkerExtension)));
        Assert.True(RayTracingPrograms.Skips(Tracing, _folder));
        Assert.False(RayTracingPrograms.Skips($"{Plain}.trim2 + sgd1_04ab63d7_t41_MSNC_VS_LinearDepthSkin_f1f2249c_Wrapped", _folder));

        // A program made in an earlier boot (or by the compile host) has only its marker; it is read again once made.
        Assert.False(RayTracingPrograms.Skips(Earlier, _folder));
        File.WriteAllBytes(Path.Combine(_folder, Earlier + RayTracingPrograms.MarkerExtension), []);
        RayTracingPrograms.Refresh(Earlier);
        Assert.True(RayTracingPrograms.Skips($"{Plain}.trim2 + {Earlier}.link-0123", _folder));
    }

    public void Dispose()
    {
        Directory.Delete(_folder, recursive: true);
    }
}
