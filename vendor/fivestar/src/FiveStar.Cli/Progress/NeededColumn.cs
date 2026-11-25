// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Cli.Progress;

/// <summary>One column of the still-needed list.</summary>
/// <param name="Area">Column heading.</param>
/// <param name="Items">The entries.</param>
public sealed record NeededColumn(string Area, IReadOnlyList<NeededItem> Items);
