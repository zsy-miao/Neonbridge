// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Compilation;

/// <summary>
/// How far a corpus run is: programs finished out of the programs it runs, and the expected cost of the finished ones
/// out of the whole (<see cref="ExpectedCosts"/>; both 0 when the run does not weigh its items).
/// </summary>
public readonly record struct CorpusProgress(int Done, int Total, double WorkDone = 0, double WorkTotal = 0);
