// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Tests.Differential;

/// <summary>The comparison of one program: its verdict and what differed or why it was skipped.</summary>
public sealed record DifferentialOutcome(string Name, string Stage, DifferentialVerdict Verdict, string Detail);
