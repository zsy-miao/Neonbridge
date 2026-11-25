// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Disassembly;

/// <summary>
/// The first instruction where two listings of one code block differ (<c>main</c> or the geometry <c>copy</c> shader);
/// a null side means that listing ended first.
/// </summary>
public sealed record DecodeMismatch(string Program, string Block, Instruction? Expected, Instruction? Actual);
