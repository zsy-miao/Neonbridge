// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Formats.Audio;

/// <summary>Reads the table-of-contents byte of an Opus packet (RFC 6716, section 3.1).</summary>
public static class OpusPacket
{
    /// <summary>Opus always counts time in samples at 48 kHz.</summary>
    public const int SampleRate = 48000;

    /// <summary>The longest packet RFC 6716 allows: 120 ms.</summary>
    public const int MaxSamples = 5760;

    private const int ConfigShift = 3;
    private const int StereoBit = 0x04;
    private const int CodeMask = 0x03;
    private const int FrameCountMask = 0x3F;
    private const int LastSilkConfig = 11;
    private const int LastHybridConfig = 15;
    private const int ArbitraryFramesCode = 3;
    private const int SizesPerMode = 4;
    private const int HybridSizes = 2;
    private const int TwoFrames = 2;

    private static readonly int[] SilkFrameSamples = [480, 960, 1920, 2880];
    private static readonly int[] HybridFrameSamples = [480, 960];
    private static readonly int[] CeltFrameSamples = [120, 240, 480, 960];

    /// <summary>Whether the packet's table-of-contents byte asks for stereo.</summary>
    public static bool IsStereo(ReadOnlySpan<byte> packet) => packet.Length > 0 && (packet[0] & StereoBit) != 0;

    /// <summary>The samples at 48 kHz the packet decodes to, or -1 when its frame count is not valid.</summary>
    public static int SampleCount(ReadOnlySpan<byte> packet)
    {
        if (packet.IsEmpty)
        {
            return -1;
        }

        var toc = packet[0];
        var frames = FrameCount(packet, toc & CodeMask);
        var samples = frames * FrameSamples(toc >> ConfigShift);

        if (frames <= 0 || samples > MaxSamples)
        {
            return -1;
        }

        return samples;
    }

    private static int FrameCount(ReadOnlySpan<byte> packet, int code)
    {
        if (code == 0)
        {
            return 1;
        }

        if (code != ArbitraryFramesCode)
        {
            return TwoFrames;
        }

        if (packet.Length < TwoFrames)
        {
            return -1;
        }

        return packet[1] & FrameCountMask;
    }

    private static int FrameSamples(int config)
    {
        if (config <= LastSilkConfig)
        {
            return SilkFrameSamples[config % SizesPerMode];
        }

        if (config <= LastHybridConfig)
        {
            return HybridFrameSamples[config % HybridSizes];
        }

        return CeltFrameSamples[config % SizesPerMode];
    }
}
