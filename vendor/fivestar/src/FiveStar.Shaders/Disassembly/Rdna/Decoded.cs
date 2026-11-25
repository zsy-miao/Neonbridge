// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Disassembly.Rdna;

/// <summary>One decoded instruction: its llvm-objdump text and its length in dwords (literal and NSA dwords included).</summary>
internal readonly record struct Decoded(string Text, int Words);
