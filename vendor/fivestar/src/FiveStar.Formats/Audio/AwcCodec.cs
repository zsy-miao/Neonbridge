// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Formats.Audio;

/// <summary>The codec byte of an AWC format entry, as identified on the dump.</summary>
public enum AwcCodec : byte
{
    /// <summary>16-bit little-endian PCM.</summary>
    Pcm16 = 0x00,

    /// <summary>32-bit little-endian IEEE float.</summary>
    PcmFloat = 0x02,

    /// <summary>XMA2 in 2 KiB packets.</summary>
    Xma2 = 0x05,

    /// <summary>Opus packets with a u16 length prefix, in 2 KiB pages.</summary>
    Opus = 0x0B,
}
