// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Cli.Setup;

/// <summary>A hash of a step's inputs (files and options) and of the tool build that processes them.</summary>
internal sealed record StepFingerprint(string Inputs, string Tool);
