// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Translation.Core;

namespace FiveStar.Shaders.Translation.Waves;

/// <summary>A translated block with its parsed statements and the blocks it passes control to.</summary>
internal sealed record WindowBlock(int Address, IReadOnlyList<WindowStatement> Statements, BlockExit Exit, IReadOnlyList<int> Successors)
{
    /// <summary>The variables the exit condition reads.</summary>
    public IReadOnlyList<string> ConditionReads => WindowStatement.Names(Exit.Condition ?? string.Empty);
}
