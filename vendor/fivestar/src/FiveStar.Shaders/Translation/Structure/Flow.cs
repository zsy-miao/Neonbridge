// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Translation.Structure;

/// <summary>
/// Where a run of emitted statements is: its scope, the exit selector of the scope's loop (null with one exit or at the
/// top level), the node where it stops, and whether falling off its end leaves the scope (the end of a loop body or of
/// the region), so that a continue or the region end needs no statement.
/// </summary>
internal sealed record Flow(ScopeGraph Scope, string? ExitSelector, int Stop, bool Tail);
