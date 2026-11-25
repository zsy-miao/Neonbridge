// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.Apu;

/// <summary>
/// The xapu.dll command types the game sends, named after what the game does around them (docs/audio-runtime.md).
/// Each command is answered by one result record of the same type.
/// </summary>
internal enum XApuCommandKind : uint
{
    /// <summary>Starts an Opus stream; the game marks the stream ready when the result arrives.</summary>
    Create = 1,

    /// <summary>Ends a stream; the result frees the game's stream number.</summary>
    Destroy = 2,

    /// <summary>Decodes one packet into the slot's output area.</summary>
    Decode = 3,

    /// <summary>Drops the stream's decoder state (a seek or a restart); the result frees the decode slot.</summary>
    Reset = 5,
}
