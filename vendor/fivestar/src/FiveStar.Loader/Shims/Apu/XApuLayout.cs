// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.Apu;

/// <summary>
/// The xapu.dll command and result records and the shared memory XApuConnect hands back. xapu.dll is not in the public
/// GDK headers: every offset is read from the executable's Opus voice code (docs/audio-runtime.md), which builds a
/// command on the stack, copies a packet into the shared memory and reads the decoded floats back from it.
/// </summary>
internal static class XApuLayout
{
    /// <summary>Bytes of one command record.</summary>
    public const int RecordSize = 0x40;

    /// <summary>Bytes of one result record: the game zeroes this much before XApuDequeueResult.</summary>
    public const int ResultSize = 0x50;

    public const int KindOffset = 0x00;
    public const int StreamOffset = 0x04;

    /// <summary>The command's identity (type, stream, sequence, options, a u64 context), echoed by its result.</summary>
    public const int IdentityBytes = 0x18;

    /// <summary>Create: the stream's channel count.</summary>
    public const int CreateChannelsOffset = 0x18;

    /// <summary>Decode: a float the voice passes (a pitch, per VIctory); not applied.</summary>
    public const int PitchOffset = 0x1C;

    /// <summary>Decode: samples per channel to drop from the start of the decoded packet.</summary>
    public const int FirstSampleOffset = 0x20;

    /// <summary>Decode: samples per channel to keep at most (960, 20 ms at 48 kHz).</summary>
    public const int FrameSamplesOffset = 0x24;

    /// <summary>Decode: packet bytes (without the AWC u16 length prefix), then the packet's address (unaligned u64).</summary>
    public const int InputBytesOffset = 0x28;
    public const int InputAddressOffset = 0x2C;

    /// <summary>Decode: output capacity in bytes (channels * 0xF00), then the output address (u64).</summary>
    public const int OutputBytesOffset = 0x34;
    public const int OutputAddressOffset = 0x38;

    /// <summary>Result: samples per channel written.</summary>
    public const int ResultSamplesOffset = 0x30;

    /// <summary>Result: bytes of 32-bit float PCM written, channels interleaved; the game scales them by 32767.</summary>
    public const int ResultBytesOffset = 0x34;

    /// <summary>Result: the output address, the pitch (1.0), the packet bytes and address, as the command gave them.</summary>
    public const int ResultOutputAddressOffset = 0x38;
    public const int ResultPitchOffset = 0x40;
    public const int ResultInputBytesOffset = 0x44;
    public const int ResultInputAddressOffset = 0x48;

    /// <summary>Decode slots in the shared memory; the game hands out slot numbers below this.</summary>
    public const int Slots = 0x80;

    /// <summary>The packet staging area of one slot, at slot * this from the start of the shared memory.</summary>
    public const int InputSlotBytes = 0x280;

    /// <summary>The output area of one slot: 960 stereo float frames.</summary>
    public const int OutputSlotBytes = 0x1E00;

    /// <summary>Where the output areas start: after the input areas of every slot.</summary>
    public const int OutputBase = Slots * InputSlotBytes;

    public const int SharedMemoryBytes = OutputBase + (Slots * OutputSlotBytes);

    /// <summary>The Opus decode rate (RFC 6716); every AWC Opus stream is decoded at it.</summary>
    public const int OpusRate = 48_000;

    /// <summary>Samples per channel of a 20 ms frame, the frame the game asks for.</summary>
    public const int DefaultFrameSamples = 960;

    /// <summary>The longest Opus packet, 120 ms at 48 kHz (RFC 6716 section 3.2.5).</summary>
    public const int MaxPacketSamples = 5_760;

    public const int MaxChannels = 2;
}
