// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Compilation;

/// <summary>One program's line in results.json: file stem, translated stage (ps, vs, cs, ms, gs or null), status and reason.</summary>
public sealed record ProgramResult(string Name, string? Stage, string Status, string? Reason);
