// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Cli.Setup;

namespace FiveStar.Cli.Tests.Support;

/// <summary>A setup step whose inputs and tool are plain text; it counts its runs and can be told to fail once.</summary>
internal sealed class FakeStep : ISetupStep
{
    public FakeStep(string name, params string[] dependsOn)
    {
        Name = name;
        DependsOn = dependsOn;
    }

    public string Name { get; }

    public string Title => "fake " + Name;

    public IReadOnlyList<string> DependsOn { get; }

    public TimeSpan Estimate(SetupContext context) => TimeSpan.FromSeconds(1);

    public string Inputs { get; set; } = "inputs";

    public string Tool { get; set; } = "tool";

    public bool FailNextRun { get; set; }

    public int Runs { get; private set; }

    public StepFingerprint Fingerprint(SetupContext context) => new(Inputs, Tool);

    public bool HasOutputs(SetupContext context) => true;

    public long RequiredBytes(SetupContext context) => 0;

    public string? MissingRequirement(SetupContext context) => null;

    public StepResult Run(SetupContext context)
    {
        Runs++;

        if (FailNextRun)
        {
            FailNextRun = false;

            throw new IOException("the process was stopped");
        }

        return StepResult.Done("ok");
    }
}
