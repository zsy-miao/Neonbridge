// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Cli.Progress;

/// <summary>One entry of the still-needed list.</summary>
/// <param name="Name">Short name.</param>
/// <param name="Note">One line on why or what blocks it, or null.</param>
public sealed record NeededItem(string Name, string? Note);
