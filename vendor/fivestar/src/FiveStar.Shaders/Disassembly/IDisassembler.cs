// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Disassembly;

/// <summary>Turns RDNA code bytes into address-accurate instructions with resolved branch targets.</summary>
public interface IDisassembler
{
    /// <summary>Disassembles code for a wave size (32 or 64; wave64 changes how lane masks decode).</summary>
    IReadOnlyList<Instruction> Disassemble(ReadOnlySpan<byte> code, int waveSize);
}
