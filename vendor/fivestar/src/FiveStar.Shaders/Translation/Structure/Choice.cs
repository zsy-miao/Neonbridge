// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Translation.Structure;

/// <summary>
/// A branching node of a scope: a conditional block, or a loop with several exits whose selector variable says which
/// exit it took. <paramref name="Successors"/> are scope-local nodes (taken target first, or the loop's exits in order);
/// the arms run up to <paramref name="Join"/>.
/// </summary>
internal sealed record Choice(ScopeNode Node, string? LoopSelector, IReadOnlyList<int> Successors, int Join);
