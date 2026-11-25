// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Formats.Audio.Xma;

namespace FiveStar.Loader.Shims.Apu;

/// <summary>
/// The 64-byte SHAPE XMA context, as the game fills and reads it (0x1400785d0 sets a voice up, 0x140078120 copies its
/// output; docs/audio-runtime.md). The fields sit where the Xbox 360 XMA context keeps them; the names follow that.
/// </summary>
internal static class XmaContextLayout
{
    /// <summary>Dword 0: 2 KiB packets of input buffer 0.</summary>
    public static readonly XmaContextField InputPackets0 = new(0, 0, 12);

    /// <summary>Dword 0: loops left; the game writes 255 for a looping voice.</summary>
    public static readonly XmaContextField LoopCount = new(0, 12, 8);

    public static readonly XmaContextField InputValid0 = new(0, 20, 1);

    public static readonly XmaContextField InputValid1 = new(0, 21, 1);

    /// <summary>Dword 0: size of the output ring in 256-byte blocks; the game writes 23 (0x1700 bytes).</summary>
    public static readonly XmaContextField OutputBlocks = new(0, 22, 5);

    /// <summary>Dword 0: the block the decoder writes next.</summary>
    public static readonly XmaContextField OutputWriteOffset = new(0, 27, 5);

    /// <summary>Dword 1: 2 KiB packets of input buffer 1; the game writes 0.</summary>
    public static readonly XmaContextField InputPackets1 = new(1, 0, 12);

    /// <summary>Dword 1: the last subframe of the loop end frame that plays.</summary>
    public static readonly XmaContextField LoopEndSubframe = new(1, 12, 2);

    /// <summary>Dword 1: subframes of the loop start frame skipped after a jump.</summary>
    public static readonly XmaContextField LoopStartSkip = new(1, 17, 3);

    /// <summary>Dword 1: subframes of the first decoded frame skipped; the game writes it when it starts mid-sound.</summary>
    public static readonly XmaContextField SubframeSkip = new(1, 24, 3);

    /// <summary>Dword 1: 0 for up to 24 kHz, 1 to 32 kHz, 2 to 44.1 kHz, 3 above.</summary>
    public static readonly XmaContextField SampleRate = new(1, 27, 2);

    public static readonly XmaContextField Stereo = new(1, 29, 1);

    /// <summary>Dword 1: set while the output ring has room; cleared when the decoder fills it.</summary>
    public static readonly XmaContextField OutputValid = new(1, 31, 1);

    /// <summary>Dword 2: where the next frame starts, in bits from the start of the current input buffer.</summary>
    public static readonly XmaContextField InputReadOffset = new(2, 0, 26);

    /// <summary>Dword 3: bit offset of the loop start frame.</summary>
    public static readonly XmaContextField LoopStart = new(3, 0, 26);

    /// <summary>Dword 4: bit offset of the loop end frame.</summary>
    public static readonly XmaContextField LoopEnd = new(4, 0, 26);

    /// <summary>Dword 4: the input buffer being decoded.</summary>
    public static readonly XmaContextField CurrentBuffer = new(4, 31, 1);

    /// <summary>Dwords 5 to 7: APU addresses of the two input buffers and the output ring.</summary>
    public static readonly XmaContextField InputBuffer0 = new(5, 0, 32);

    public static readonly XmaContextField InputBuffer1 = new(6, 0, 32);

    public static readonly XmaContextField OutputBuffer = new(7, 0, 32);

    /// <summary>Dword 8: APU address of the context's work buffer (<see cref="WorkBufferBytes"/>).</summary>
    public static readonly XmaContextField WorkBuffer = new(8, 0, 32);

    /// <summary>Dword 9: the block the game reads next; it moves when the game sends message 0x12.</summary>
    public static readonly XmaContextField OutputReadOffset = new(9, 0, 5);

    /// <summary>The sample rates <see cref="SampleRate"/> selects.</summary>
    public static readonly int[] SampleRates = [24_000, 32_000, 44_100, 48_000];

    /// <summary>A loop count that never runs down.</summary>
    public const uint InfiniteLoops = 255;

    /// <summary>Bytes of a work buffer: the game cuts one 0x200-byte buffer per context (0x1443aed13).</summary>
    public const int WorkBufferBytes = 0x200;

    /// <summary>Bytes of an output block: one 128-sample subframe of one channel in 16-bit PCM.</summary>
    public const int BlockBytes = XmaLayout.MinSubframeSamples * sizeof(short);
}
