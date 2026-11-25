// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Cpu;

/// <summary>
/// One EXTRQ or INSERTQ check of the self-test.
/// </summary>
/// <param name="Name">What the log calls it.</param>
/// <param name="Insert">INSERTQ rather than EXTRQ.</param>
/// <param name="Field">The immediate form's field; null for the register form, whose control is in the source.</param>
/// <param name="Extended">Use xmm9 and xmm10 (REX) instead of xmm1 and xmm2.</param>
/// <param name="SourceLow">The source's low quadword: INSERTQ's bits, EXTRQ's register-form control.</param>
/// <param name="SourceHigh">The source's high quadword: INSERTQ's register-form control.</param>
internal sealed record Sse4aCase(string Name, bool Insert, BitFieldSpec? Field, bool Extended, ulong SourceLow, ulong SourceHigh);
