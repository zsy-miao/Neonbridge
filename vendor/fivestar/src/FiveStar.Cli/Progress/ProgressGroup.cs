// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Text.Json.Serialization;

namespace FiveStar.Cli.Progress;

/// <summary>One tile of a progress area: items done, done only through a fallback or stand-in, and not done yet.</summary>
/// <param name="Name">Short label drawn on the tile.</param>
/// <param name="Done">Items done.</param>
/// <param name="Partial">Items done only through a fallback or stand-in.</param>
/// <param name="Todo">Items not done yet.</param>
/// <param name="Note">Extra text for the hover title, or null.</param>
public sealed record ProgressGroup(string Name, double Done, double Partial, double Todo, string? Note)
{
    [JsonIgnore]
    public double Total => Done + Partial + Todo;
}
