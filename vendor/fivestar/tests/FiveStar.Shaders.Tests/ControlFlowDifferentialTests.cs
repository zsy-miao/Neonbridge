// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Text.Json;
using FiveStar.Shaders.Tests.Differential;
using FiveStar.Shaders.Tests.Toolchain;
using FiveStar.Shaders.Translation;
using Xunit;

namespace FiveStar.Shaders.Tests;

/// <summary>
/// The structured control-flow form against the dispatch loop on real programs of the user's dump: both forms run on
/// WARP with the same synthetic inputs and every output word must be equal. FIVESTAR_DIFF_PROGRAMS sets the programs
/// per stage (default 3) and FIVESTAR_DIFF_FIRST how many of the stage's sample to pass over first (to resume a run that WARP
/// stalled), FIVESTAR_DIFF_STAGE runs one stage only; FIVESTAR_DIFF_OUT names a folder that keeps the built shaders and gets differential.jsonl,
/// one outcome per line.
/// </summary>
public sealed class ControlFlowDifferentialTests : IClassFixture<WarpFixture>
{
    public const string ProgramsVariable = "FIVESTAR_DIFF_PROGRAMS";
    public const string FirstVariable = "FIVESTAR_DIFF_FIRST";
    public const string StageVariable = "FIVESTAR_DIFF_STAGE";
    public const string OutputVariable = "FIVESTAR_DIFF_OUT";

    private const int DefaultPrograms = 3;
    private const int Jobs = 4;

    private readonly WarpFixture _fixture;

    public ControlFlowDifferentialTests(WarpFixture fixture)
    {
        _fixture = fixture;
    }

    [RealProgramTheory]
    [InlineData("cs")]
    [InlineData("ps")]
    [InlineData("vs")]
    [InlineData("ms")]
    public void Both_control_flow_forms_of_a_real_program_give_the_same_outputs(string stage)
    {
        var only = Environment.GetEnvironmentVariable(StageVariable);

        if (!string.IsNullOrWhiteSpace(only) && only != stage)
        {
            return;
        }

        var count = DefaultPrograms;

        if (int.TryParse(Environment.GetEnvironmentVariable(ProgramsVariable), out var programs))
        {
            count = programs;
        }

        _ = int.TryParse(Environment.GetEnvironmentVariable(FirstVariable), out var first);
        var output = Environment.GetEnvironmentVariable(OutputVariable);
        var folder = Path.Combine(_fixture.Folder, "differential");

        if (!string.IsNullOrWhiteSpace(output))
        {
            folder = Path.Combine(output, stage);
        }

        var runner = new DifferentialRunner(_fixture, folder, backend: false);
        var sample = ProgramSample.Of(RealProgramTheoryAttribute.Folder!, ShaderStages.FromProfileName(stage)!.Value, first + count).Skip(first).ToList();
        var outcomes = new DifferentialOutcome[sample.Count];
        var report = new Lock();
        Parallel.For(0, sample.Count, new ParallelOptions { MaxDegreeOfParallelism = Jobs }, index =>
        {
            var outcome = runner.Run(sample[index]);
            outcomes[index] = outcome;

            if (string.IsNullOrWhiteSpace(output))
            {
                return;
            }

            var line = JsonSerializer.Serialize(new { Index = first + index, outcome.Name, outcome.Stage, Verdict = outcome.Verdict.ToString(), outcome.Detail });

            lock (report)
            {
                File.AppendAllLines(Path.Combine(output, "differential.jsonl"), [line]);
            }
        });

        var mismatches = outcomes.Where(outcome => outcome.Verdict == DifferentialVerdict.Mismatch).ToList();

        Assert.True(mismatches.Count == 0, string.Join("\n", mismatches.Select(outcome => $"{outcome.Name}: {outcome.Detail}")));
    }
}
