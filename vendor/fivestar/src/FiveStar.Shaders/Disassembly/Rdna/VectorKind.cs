// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Disassembly.Rdna;

/// <summary>The 32-bit vector ALU encodings.</summary>
internal enum VectorKind
{
    /// <summary>One source: VDST [24:17], OP [16:9], SRC0 [8:0].</summary>
    Vop1,

    /// <summary>Two sources: OP [30:25], VDST [24:17], VSRC1 [16:9], SRC0 [8:0].</summary>
    Vop2,

    /// <summary>Compare into VCC: OP [24:17], VSRC1 [16:9], SRC0 [8:0].</summary>
    Vopc,
}
