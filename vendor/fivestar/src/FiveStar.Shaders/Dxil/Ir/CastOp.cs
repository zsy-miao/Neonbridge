// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Dxil.Ir;

/// <summary>LLVM cast codes.</summary>
public enum CastOp
{
    Trunc = 0,
    ZExt = 1,
    SExt = 2,
    FpToUi = 3,
    FpToSi = 4,
    UiToFp = 5,
    SiToFp = 6,
    FpTrunc = 7,
    FpExt = 8,
    Bitcast = 11,
}
