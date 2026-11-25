// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.Apu;

/// <summary>The effect kind at +0x00 of an XDspActivate config, as the game sets it.</summary>
internal enum XDspEffectKind
{
    /// <summary>A convolution with an impulse response from the game (pointer and length in the config).</summary>
    Convolution = 1,

    /// <summary>Activated with 1024-frame mono blocks and nothing else (0x1443b3920); what it computes is not known.</summary>
    Block = 2,
}
