// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Text.Json;
using FiveStar.Shaders.Tests.Differential;
using FiveStar.Shaders.Tests.Toolchain;
using FiveStar.Shaders.Translation;
using Xunit;

namespace FiveStar.Shaders.Tests;

/// <summary>
/// The DXIL backend against dxc on real programs of the user's dump: the same structured translation compiled both ways
/// runs on WARP with the same synthetic inputs (pixel shaders drawn with their feeder, vertex shaders in their compute
/// harness), and every output word must be equal (a difference that goes
/// away when dxc compiles with -Gis is dxc's fast-math, since the backend keeps IEEE operations). The variables of
/// <see cref="ControlFlowDifferentialTests"/> size the sample; FIVESTAR_DIFF_OUT also gets backend.jsonl.
/// </summary>
public sealed class DxilBackendDifferentialTests : IClassFixture<WarpFixture>
{
    private const int DefaultPrograms = 3;
    private const int Jobs = 4;

    private readonly WarpFixture _fixture;

    public DxilBackendDifferentialTests(WarpFixture fixture)
    {
        _fixture = fixture;
    }

    [RealProgramTheory]
    [InlineData("cs")]
    [InlineData("ps")]
    [InlineData("vs")]
    public void The_dxil_backend_and_dxc_give_the_same_outputs_on_real_programs(string stage)
    {
        var only = Environment.GetEnvironmentVariable(ControlFlowDifferentialTests.StageVariable);

        // One stage per run: a WARP stall stops every later run in the process.
        if (!string.IsNullOrWhiteSpace(only) && only != stage)
        {
            return;
        }

        var count = DefaultPrograms;

        if (int.TryParse(Environment.GetEnvironmentVariable(ControlFlowDifferentialTests.ProgramsVariable), out var programs))
        {
            count = programs;
        }

        _ = int.TryParse(Environment.GetEnvironmentVariable(ControlFlowDifferentialTests.FirstVariable), out var first);
        var output = Environment.GetEnvironmentVariable(ControlFlowDifferentialTests.OutputVariable);
        var folder = Path.Combine(_fixture.Folder, "backend");

        if (!string.IsNullOrWhiteSpace(output))
        {
            folder = Path.Combine(output, "backend-" + stage);
        }

        var runner = new DifferentialRunner(_fixture, folder, backend: true);
        var sample = ProgramSample.Of(RealProgramTheoryAttribute.Folder!, ShaderStages.FromProfileName(stage)!.Value, first + count).Skip(first).ToList();
        var outcomes = new DifferentialOutcome[sample.Count];
        var report = new Lock();
        Parallel.For(0, sample.Count, new ParallelOptions { MaxDegreeOfParallelism = Jobs }, index =>
        {
            outcomes[index] = runner.Run(sample[index]);

            if (string.IsNullOrWhiteSpace(output))
            {
                return;
            }

            var outcome = outcomes[index];
            var line = JsonSerializer.Serialize(new { Index = first + index, outcome.Name, outcome.Stage, Verdict = outcome.Verdict.ToString(), outcome.Detail });

            lock (report)
            {
                File.AppendAllLines(Path.Combine(output, "backend.jsonl"), [line]);
            }
        });

        var mismatches = outcomes.Where(outcome => outcome.Verdict == DifferentialVerdict.Mismatch).ToList();

        Assert.True(mismatches.Count == 0, string.Join("\n", mismatches.Select(outcome => $"{outcome.Name}: {outcome.Detail}")));
    }
}
