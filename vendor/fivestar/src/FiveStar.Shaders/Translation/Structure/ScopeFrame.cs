// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Translation.Structure;

/// <summary>
/// A scope being emitted: its number, the emitted block-line count when it started, the most block lines it may emit,
/// and whether it is already in the guarded form (which copies nothing, so it is not limited).
/// </summary>
internal sealed record ScopeFrame(int Id, int Start, int Limit, bool Guarded);
