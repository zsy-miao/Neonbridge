// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Cli.Progress;

/// <summary>One section of the report: a headline and a treemap of its groups.</summary>
/// <param name="Key">Stable identifier (<c>shaders</c>, <c>textures</c>...); a newer area replaces an older one with the same key.</param>
/// <param name="Title">Headline text before the percentage.</param>
/// <param name="Unit">Unit appended to amounts (<c>" GiB"</c>), or null for plain counts.</param>
/// <param name="Height">Treemap height in pixels, or null for the default.</param>
/// <param name="Groups">The tiles.</param>
public sealed record ProgressArea(string Key, string Title, string? Unit, double? Height, IReadOnlyList<ProgressGroup> Groups);
