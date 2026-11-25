// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Formats.Audio.Xma;

/// <summary>
/// Decodes a whole XMA2 sound, a run of 2 KiB packets of one stream, into interleaved float PCM. The decoded run is
/// trimmed the way FFmpeg's XMA2 decoder trims it: one frame plus 64 samples of decoder delay at the start, and at the
/// end the last frame's end trim plus the same 64 samples (docs/xma.md). The console's own alignment is not known.
/// </summary>
public static class XmaTrackDecoder
{
    /// <summary>Samples dropped at the start: the first frame, which only primes the overlap, and the decoder delay.</summary>
    public const int StartTrim = XmaLayout.FrameSamples + DecoderDelay;

    /// <summary>The delay of the XMA decoder beyond its first frame, in samples (FFmpeg's value).</summary>
    public const int DecoderDelay = 64;

    private const int Growth = 2;

    public static XmaDecodedSound Decode(ReadOnlySpan<byte> data, int channels, int sampleRate)
    {
        var decoder = new XmaStreamDecoder(channels, sampleRate);
        var packet = new float[decoder.MaxPacketOutput];
        var frameFloats = XmaLayout.FrameSamples * channels;
        var samples = new float[Math.Max(data.Length / XmaLayout.PacketBytes, 1) * frameFloats];
        var used = 0;

        for (var offset = 0; offset + XmaLayout.PacketBytes <= data.Length; offset += XmaLayout.PacketBytes)
        {
            var frames = decoder.DecodePacket(data.Slice(offset, XmaLayout.PacketBytes), packet);
            Append(ref samples, ref used, packet.AsSpan(0, frames * frameFloats));
        }

        var tail = decoder.Flush(packet);
        Append(ref samples, ref used, packet.AsSpan(0, tail * channels));

        var start = Math.Min(StartTrim * channels, used);
        var end = Math.Max(start, used - ((DecoderDelay + decoder.TrimEnd) * channels));

        return new XmaDecodedSound(samples[start..end], channels, sampleRate, decoder.Frames, decoder.BadFrames + decoder.LostFrames);
    }

    private static void Append(ref float[] samples, ref int used, ReadOnlySpan<float> more)
    {
        if (used + more.Length > samples.Length)
        {
            Array.Resize(ref samples, Math.Max(samples.Length * Growth, used + more.Length));
        }

        more.CopyTo(samples.AsSpan(used));
        used += more.Length;
    }
}
