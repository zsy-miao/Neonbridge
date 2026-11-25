// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Cli.Setup;

/// <summary>How a step run ended, with a one-line summary of what it made or why it failed.</summary>
internal sealed record StepResult(bool Succeeded, string Summary)
{
    public static StepResult Done(string summary) => new(true, summary);

    public static StepResult Failed(string reason) => new(false, reason);
}
