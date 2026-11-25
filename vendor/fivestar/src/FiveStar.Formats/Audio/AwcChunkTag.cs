// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Formats.Audio;

/// <summary>The tag byte (bits 56..63) of an AWC chunk entry. Only <see cref="Data"/> and the format chunks are read.</summary>
public enum AwcChunkTag : byte
{
    /// <summary>Encoded audio.</summary>
    Data = 0x55,

    /// <summary>The 0x20-byte format of a single stream: sample count, sample rate and codec.</summary>
    Format = 0x76,

    /// <summary>The block layout and channel list of a multichannel stream.</summary>
    StreamFormat = 0x48,

    /// <summary>The start sample of every 2 KiB page, and one entry past the last.</summary>
    SeekTable = 0xA3,

    /// <summary>Peak levels.</summary>
    Peak = 0x36,

    /// <summary>Markers.</summary>
    Markers = 0xBD,

    /// <summary>Animation.</summary>
    Animation = 0x5C,

    /// <summary>Granular synthesis grains.</summary>
    GranularGrains = 0x5A,

    /// <summary>Granular synthesis loops.</summary>
    GranularLoops = 0xD9,
}
