// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Cli.Progress;

/// <summary>
/// The content of <c>status.json</c> (and of the hand-written manual file, which has the same shape): the areas in
/// drawing order and the still-needed columns.
/// </summary>
/// <param name="Areas">Areas in drawing order.</param>
/// <param name="StillNeeded">Still-needed columns, or null.</param>
public sealed record ProgressStatus(IReadOnlyList<ProgressArea> Areas, IReadOnlyList<NeededColumn>? StillNeeded);
