// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Graphics.Programs;

/// <summary>
/// The form of a program an adapter runs: the DXIL files (one, or the fallback's compute then vertex shader) and why a
/// form other than the native one was picked or why none is correct.
/// </summary>
public sealed record ProgramChoice(ProgramVariants Program, ChoiceOutcome Outcome, IReadOnlyList<string> Files, string? Reason);
