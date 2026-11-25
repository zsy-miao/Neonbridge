// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Cli.Exe;

/// <summary>One entry of the day-one gap list: something that blocks the game from running.</summary>
/// <param name="Area">The area it belongs to (Loader, Graphics, Archives, GDK).</param>
/// <param name="What">What is missing.</param>
/// <param name="Why">Why it blocks running, with the evidence from this exe.</param>
public sealed record GapItem(string Area, string What, string Why);
