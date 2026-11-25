// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Cli.Setup;

/// <summary>Where a step stands against its recorded state.</summary>
internal enum StepCondition
{
    /// <summary>Finished with the current inputs and tool; a run skips it.</summary>
    Done,

    NotRun,

    /// <summary>Started and not finished: the process ended or the step failed.</summary>
    Interrupted,

    InputsChanged,

    ToolChanged,

    OutputsMissing,

    /// <summary>A step it depends on is not done.</summary>
    Waiting,
}
