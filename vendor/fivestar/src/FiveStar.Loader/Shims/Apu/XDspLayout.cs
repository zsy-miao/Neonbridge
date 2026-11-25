// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.Apu;

/// <summary>
/// The chad.dll (XDsp) records. chad.dll is not in the public GDK headers: every offset is read from the executable's
/// effect code (docs/audio-runtime.md), which fills an activation config, submits 0x20-byte commands and polls a
/// status record the activation hands back.
/// </summary>
internal static class XDspLayout
{
    /// <summary>Activation config: the effect kind (<see cref="XDspEffectKind"/>).</summary>
    public const int ConfigKindOffset = 0x00;

    /// <summary>Activation config: frames per processed block (the game always asks for 1024).</summary>
    public const int ConfigFramesOffset = 0x08;

    /// <summary>Activation config: the impulse response's channels, 1 or 2, for a convolution (0x14003b928); 1 for kind 2.</summary>
    public const int ConfigChannelsOffset = 0x0C;

    /// <summary>Convolution config: impulse response length in 4-byte units (its byte size / 4), then its address (unaligned u64).</summary>
    public const int ConfigResponseLengthOffset = 0x10;
    public const int ConfigResponseAddressOffset = 0x14;

    /// <summary>Bytes of an activation config the shim reads: up to the end of the response address.</summary>
    public const int ConfigSize = ConfigResponseAddressOffset + sizeof(long);

    /// <summary>Bytes of one command record.</summary>
    public const int CommandSize = 0x20;

    /// <summary>Command: two floats the game always sets to 1.0 (0x145fc6ed8), read here as gains.</summary>
    public const int CommandGain0Offset = 0x00;
    public const int CommandGain1Offset = 0x04;

    /// <summary>Command: input block address, then output block address (u64 each).</summary>
    public const int CommandInputOffset = 0x08;
    public const int CommandOutputOffset = 0x10;

    /// <summary>Bytes of one input or output page of the game's pool (0x141d6bdf0): 1024 floats.</summary>
    public const int PageBytes = 0x1000;

    /// <summary>
    /// Frames per channel of a two-channel command: the game's mixer block, 256 frames (0x144e5a5be passes 0x100), each
    /// channel's block after the other's in the page (0x1443b2a40). A one-channel command carries the configured frames.
    /// </summary>
    public const int StereoBlockFrames = 0x100;

    /// <summary>The most channels a stream convolves.</summary>
    public const int MaxChannels = 2;

    /// <summary>
    /// The status record of one stream. The game reads the u32 at <see cref="StatusCompletedOffset"/>, compares it
    /// with the id the last submit returned, and takes results only while the u32 at <see cref="StatusErrorOffset"/>
    /// is 0.
    /// </summary>
    public const int StatusSize = 0x40;
    public const int StatusCompletedOffset = 0x04;
    public const int StatusErrorOffset = 0x0C;

    /// <summary>The most streams one connection holds; the game asks for 0x18 or 0x1E.</summary>
    public const uint MaxStreams = 0x100;
}
