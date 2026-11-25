// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Disassembly;

namespace FiveStar.Shaders.Tests.Assembly;

/// <summary>A handwritten program after llvm-mc: its code bytes and the instruction list the translator consumes.</summary>
public sealed record AssembledProgram(byte[] Code, IReadOnlyList<Instruction> Instructions);
