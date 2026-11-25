// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Cli.Setup;

/// <summary>The record of a finished step: the fingerprint it ran with, when it finished, how long it took, its summary.</summary>
internal sealed record StepState(string Step, string Inputs, string Tool, DateTimeOffset Finished, TimeSpan Duration, string Summary);
