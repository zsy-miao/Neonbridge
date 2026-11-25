// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Formats.Ttpf.BcPack;

/// <summary>The lane engines of <see cref="BcPackDecoder"/>; all give identical results.</summary>
public enum BcPackEngine
{
    /// <summary>Portable, one lane at a time.</summary>
    Scalar,

    /// <summary>AVX2 and BMI2: two 8-lane vectors per clock.</summary>
    Avx2,

    /// <summary>AVX-512 F/BW/VL/VBMI2 and BMI2: all lanes in one 16-lane vector, kept in registers across clocks.</summary>
    Avx512,
}
