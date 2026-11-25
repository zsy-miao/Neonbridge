// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Translation.Emitters;

/// <summary>How a formatted buffer access carries its components: whole VGPRs, or d16 halves from the low or (d16_hi) the high half.</summary>
internal enum HalfKind
{
    None,
    Low,
    High,
}
