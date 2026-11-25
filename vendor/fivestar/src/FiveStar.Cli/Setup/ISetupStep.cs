// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Cli.Setup;

/// <summary>
/// One step of <c>fivestar setup</c>. A step is redone when its fingerprint or that of a step it depends on changed,
/// or when its outputs are gone; otherwise a re-run skips it.
/// </summary>
internal interface ISetupStep
{
    /// <summary>The short name used in state files and <c>--only</c>.</summary>
    string Name { get; }

    string Title { get; }

    /// <summary>The steps whose outputs this one reads; they come earlier in the step list.</summary>
    IReadOnlyList<string> DependsOn { get; }

    /// <summary>The time of a first run, for the estimate until a run has been measured.</summary>
    TimeSpan Estimate(SetupContext context);

    /// <summary>The inputs and tool version; reads only what is there and never throws for missing inputs.</summary>
    StepFingerprint Fingerprint(SetupContext context);

    bool HasOutputs(SetupContext context);

    /// <summary>The free disk space the step needs in the work folder.</summary>
    long RequiredBytes(SetupContext context);

    /// <summary>What the step needs and cannot find (a tool), or null.</summary>
    string? MissingRequirement(SetupContext context);

    StepResult Run(SetupContext context);
}
