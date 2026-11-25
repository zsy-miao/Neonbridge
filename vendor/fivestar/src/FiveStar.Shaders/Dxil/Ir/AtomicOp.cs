// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Dxil.Ir;

/// <summary>LLVM <c>atomicrmw</c> operations (shared memory atomics).</summary>
public enum AtomicOp
{
    Exchange = 0,
    Add = 1,
    Sub = 2,
    And = 3,
    Or = 5,
    Xor = 6,
    Max = 7,
    Min = 8,
    UMax = 9,
    UMin = 10,
}
