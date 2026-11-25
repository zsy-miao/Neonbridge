// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Cli.Setup;
using FiveStar.Cli.Shader;
using FiveStar.Cli.Tests.Support;
using Xunit;

namespace FiveStar.Cli.Tests;

/// <summary>`fivestar setup` skips finished steps, redoes what changed with what depends on it, and resumes an interrupted run.</summary>
public sealed class SetupRunnerTests : IDisposable
{
    private readonly string _work = Path.Combine(Path.GetTempPath(), "fivestar-setup-" + Guid.NewGuid().ToString("N"));
    private readonly FakeStep _extract = new("extract");
    private readonly FakeStep _translate;
    private readonly FakeStep _pipelines;
    private readonly FakeStep _textures;

    public SetupRunnerTests()
    {
        _translate = new FakeStep("translate", "extract");
        _pipelines = new FakeStep("pipelines", "translate");
        _textures = new FakeStep("textures", "extract");
    }

    public void Dispose()
    {
        if (Directory.Exists(_work))
        {
            Directory.Delete(_work, recursive: true);
        }
    }

    [Fact]
    public void A_rerun_skips_finished_steps_and_redoes_a_changed_step_with_the_steps_after_it()
    {
        Assert.True(Run());
        Assert.Equal([1, 1, 1, 1], Runs());

        Assert.True(Run());
        Assert.Equal([1, 1, 1, 1], Runs());

        _translate.Inputs = "new programs";
        Assert.Equal([StepCondition.Done, StepCondition.InputsChanged, StepCondition.Waiting, StepCondition.Done], Runner().Check(Context()));
        Assert.True(Run());
        Assert.Equal([1, 2, 2, 1], Runs());

        _textures.Tool = "new build";
        Assert.True(Run());
        Assert.Equal([1, 2, 2, 2], Runs());
    }

    [Fact]
    public void An_interrupted_run_resumes_at_the_step_that_did_not_finish()
    {
        _translate.FailNextRun = true;

        Assert.False(Run());
        Assert.Equal([1, 1, 0, 1], Runs());
        Assert.Equal(StepCondition.Interrupted, Runner().Check(Context())[1]);

        Assert.True(Run());
        Assert.Equal([1, 2, 1, 1], Runs());
        Assert.All(Runner().Check(Context()), condition => Assert.Equal(StepCondition.Done, condition));
    }

    private bool Run() => Runner().Run(Context(), null, force: false);

    private int[] Runs() => [_extract.Runs, _translate.Runs, _pipelines.Runs, _textures.Runs];

    private SetupRunner Runner() =>
        new([_extract, _translate, _pipelines, _textures], new StateStore(Path.Combine(_work, "state")), TextWriter.Null, TimeProvider.System, () => long.MaxValue);

    private SetupContext Context() => new(_work, _work, null, 1, 1, new CommandArguments([], new HashSet<string>()));
}
