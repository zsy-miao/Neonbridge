// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Tests.Differential;
using FiveStar.Shaders.Tests.Toolchain;
using FiveStar.Shaders.Translation;
using Xunit;
using Xunit.Abstractions;

namespace FiveStar.Shaders.Tests;

/// <summary>
/// Fast waves (TranslationOptions.FastWaves) on real programs of the user's dump: the first fast-waves mesh programs of
/// the sample (FIVESTAR_DIFF_PROGRAMS, default 3) run in that form and in their superstep form on WARP with the same
/// synthetic inputs. Both forms must build and run; the form is approximate, so the outputs that differ are reported
/// (and written to FIVESTAR_DIFF_OUT/fast-waves.txt), not failed.
/// </summary>
public sealed class FastWavesDifferentialTests : IClassFixture<WarpFixture>
{
    private const int DefaultPrograms = 3;

    // Programs looked at per one wanted: about three in ten mesh programs need the superstep form.
    private const int Candidates = 12;

    private readonly WarpFixture _fixture;
    private readonly ITestOutputHelper _output;

    public FastWavesDifferentialTests(WarpFixture fixture, ITestOutputHelper output)
    {
        _fixture = fixture;
        _output = output;
    }

    [RealProgramTheory]
    [InlineData("ms")]
    public void Fast_waves_programs_build_and_run_beside_their_superstep_form(string stage)
    {
        var count = DefaultPrograms;

        if (int.TryParse(Environment.GetEnvironmentVariable(ControlFlowDifferentialTests.ProgramsVariable), out var programs))
        {
            count = programs;
        }

        var output = Environment.GetEnvironmentVariable(ControlFlowDifferentialTests.OutputVariable);
        var folder = Path.Combine(_fixture.Folder, "fast-waves");

        if (!string.IsNullOrWhiteSpace(output))
        {
            folder = Path.Combine(output, "fast-waves");
        }

        var runner = new DifferentialRunner(_fixture, folder, backend: false);
        var outcomes = new List<DifferentialOutcome>();

        foreach (var path in ProgramSample.Of(RealProgramTheoryAttribute.Folder!, ShaderStages.FromProfileName(stage)!.Value, count * Candidates))
        {
            var outcome = runner.RunFastWaves(path);

            if (outcome.Detail is "not a fast-waves program")
            {
                continue;
            }

            outcomes.Add(outcome);
            _output.WriteLine($"{outcome.Name}: {outcome.Verdict} {outcome.Detail}");

            if (outcomes.Count == count)
            {
                break;
            }
        }

        if (!string.IsNullOrWhiteSpace(output))
        {
            File.WriteAllLines(Path.Combine(folder, "fast-waves.txt"), outcomes.Select(outcome => $"{outcome.Name}\t{outcome.Verdict}\t{outcome.Detail}"));
        }

        var broken = outcomes.Where(outcome => outcome.Detail.StartsWith("build:", StringComparison.Ordinal) || outcome.Detail.StartsWith("run:", StringComparison.Ordinal)).ToList();

        Assert.True(broken.Count == 0, string.Join("\n", broken.Select(outcome => $"{outcome.Name}: {outcome.Detail}")));
    }
}
