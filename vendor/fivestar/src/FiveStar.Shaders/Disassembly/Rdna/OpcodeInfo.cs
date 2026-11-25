// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Disassembly.Rdna;

/// <summary>
/// One opcode of an encoding: its mnemonic, the types of its destination and sources, and whether llvm-objdump leaves
/// the encoding suffix (<c>_e32</c>/<c>_e64</c>) off because the instruction has only one encoding.
/// </summary>
internal sealed record OpcodeInfo(string Name, OperandType Destination, IReadOnlyList<OperandType> Sources, bool Bare);
