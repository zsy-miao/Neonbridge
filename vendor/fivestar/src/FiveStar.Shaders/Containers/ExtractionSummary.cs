// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Containers;

/// <summary>Counts of one archive extraction: records found, unique containers written, programs written and failed.</summary>
public sealed record ExtractionSummary(int Records, int Containers, int Programs, int ProgramFailures);
