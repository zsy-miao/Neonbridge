// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.Apu;

/// <summary>The IAcpHal::SendMessage types the game sends (docs/audio-runtime.md).</summary>
internal enum AcpHalMessage : uint
{
    /// <summary>At start-up, payload 0xC0; meaning unknown.</summary>
    StartUp = 2,

    /// <summary>Payload: the context index. Starts decoding a context the game has filled (0x1443a73e0).</summary>
    EnableContext = 5,

    /// <summary>Payload: the context index. Stops a context when its voice ends (0x1400925e0).</summary>
    DisableContext = 7,

    /// <summary>Payload: the context index and the output blocks the game took (0x140078120).</summary>
    ConsumeOutput = 0x12,
}
