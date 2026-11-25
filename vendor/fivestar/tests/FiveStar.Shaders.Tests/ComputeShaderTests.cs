// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Tests.Cases;
using FiveStar.Shaders.Tests.Toolchain;
using Xunit;

namespace FiveStar.Shaders.Tests;

/// <summary>
/// Handwritten compute programs (thread / group ids, user SGPRs, LDS, wave operations as wave32 and wave64, buffer and
/// global memory, atomics) translated, run on WARP and compared with the ISA reference model per thread.
/// </summary>
public sealed class ComputeShaderTests : IClassFixture<WarpFixture>
{
    private const string FullWaveReason = "needs a PC wave as wide as the AMD wave (32 or 64 lanes); WARP runs 4-lane waves";

    private readonly WarpFixture _fixture;

    public ComputeShaderTests(WarpFixture fixture)
    {
        _fixture = fixture;
    }

    public static TheoryData<string> Cases => [.. ComputeCases.All().Select(testCase => testCase.Name)];

    public static TheoryData<string> BackendCases => [.. ComputeCases.All().Where(testCase => !testCase.DxcOnly).Select(testCase => testCase.Name)];

    public static TheoryData<string> FullWaveCases => [.. ComputeCases.FullWave().Select(testCase => testCase.Name)];

    public static TheoryData<string> Wave64Cases =>
        [.. ComputeWaveCases.All().Concat(ComputeWaveCases.FullWave()).Where(testCase => testCase.Wave == 64).Select(testCase => testCase.Name)];

    [WarpTheory]
    [MemberData(nameof(Wave64Cases))]
    public void A_wave64_program_runs_as_its_thread_group_on_narrower_waves(string name)
    {
        var mismatches = new ComputeCaseRunner(_fixture).RunGroupWave(ComputeCases.Find(name));

        Assert.True(mismatches.Count == 0, $"{mismatches.Count} mismatches:\n{string.Join("\n", mismatches.Take(16))}");
    }

    // Raw programs number their threads for 64-thread groups; ds_permute's loop over lanes has no superstep form.
    // Each with and without the liveness carry.
    public static TheoryData<string, bool> SuperstepCases
    {
        get
        {
            var data = new TheoryData<string, bool>();
            var cases = ComputeWaveCases.All().Concat(ComputeWaveCases.FullWave())
                .Where(testCase => testCase.Wave == 64 && !testCase.Raw && !testCase.Name.StartsWith("cs_ds_permute", StringComparison.Ordinal));

            foreach (var testCase in cases)
            {
                data.Add(testCase.Name, false);
                data.Add(testCase.Name, true);
            }

            return data;
        }
    }

    [WarpTheory]
    [MemberData(nameof(SuperstepCases))]
    public void A_wave64_program_runs_in_supersteps_with_several_waves_per_group(string name, bool carry)
    {
        var mismatches = new ComputeCaseRunner(_fixture).RunSupersteps(ComputeCases.Find(name), carry);

        Assert.True(mismatches.Count == 0, $"{mismatches.Count} mismatches:\n{string.Join("\n", mismatches.Take(16))}");
    }

    [WarpTheory]
    [MemberData(nameof(BackendCases))]
    public void The_dxil_backend_computes_what_the_isa_defines(string name)
    {
        var mismatches = new ComputeCaseRunner(_fixture).RunDirect(ComputeCases.Find(name));

        Assert.True(mismatches.Count == 0, $"{mismatches.Count} mismatches:\n{string.Join("\n", mismatches.Take(16))}");
    }

    [WarpTheory]
    [MemberData(nameof(Cases))]
    [MemberData(nameof(FullWaveCases), Skip = FullWaveReason)]
    public void A_compute_program_computes_what_the_isa_defines(string name)
    {
        var mismatches = new ComputeCaseRunner(_fixture).Run(ComputeCases.Find(name));

        Assert.True(mismatches.Count == 0, $"{mismatches.Count} mismatches:\n{string.Join("\n", mismatches.Take(16))}");
    }
}
