// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Translation.Core;

/// <summary>
/// The end of a basic block. The block's last line is the <c>pc</c> assignment that the dispatch form runs; the
/// structured form uses this description instead. <paramref name="Target"/> is the branch target (Jump, Conditional),
/// <paramref name="Next"/> the fall-through address of a conditional branch, <paramref name="Condition"/> its wave-uniform
/// HLSL condition.
/// </summary>
internal sealed record BlockExit(BlockExitKind Kind, long? Target, long? Next, string? Condition);
