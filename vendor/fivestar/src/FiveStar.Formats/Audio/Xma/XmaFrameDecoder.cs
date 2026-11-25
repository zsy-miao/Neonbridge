// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Formats.Audio.Xma;

/// <summary>
/// Decodes one WMA Pro frame of an XMA stream into 512 samples per channel. A frame is: its length, the subframe
/// tiling, an unused post-processing transform, a dynamic range byte, optional trim counts, then the subframes in
/// time order, each a set of channels with the same offset and length (docs/xma.md).
/// </summary>
internal sealed class XmaFrameDecoder
{
    private const int PostTransformBits = 4;
    private const int TrailerBits = 2;

    private readonly XmaChannel[] _channels;
    private readonly int _channelCount;
    private readonly XmaBandLayout _bands;
    private readonly XmaSubframeDecoder _subframes;

    public XmaFrameDecoder(int channels, int sampleRate)
    {
        if (channels < 1 || channels > XmaLayout.MaxChannels)
        {
            throw new ArgumentOutOfRangeException(nameof(channels), channels, "an XMA stream has one or two channels");
        }

        _channelCount = channels;
        _channels = new XmaChannel[channels];

        for (var index = 0; index < channels; index++)
        {
            _channels[index] = new XmaChannel();
        }

        _bands = new XmaBandLayout(sampleRate);
        _subframes = new XmaSubframeDecoder(_channels, _bands);
    }

    public int Channels => _channelCount;

    /// <summary>Gets the start trim of the last frame that carried one, in samples.</summary>
    public int TrimStart { get; private set; }

    /// <summary>Gets the end trim of the last frame that carried one, in samples.</summary>
    public int TrimEnd { get; private set; }

    /// <summary>
    /// Decodes the frame in the first <paramref name="lengthBits"/> bits of <paramref name="frame"/> and writes 512
    /// interleaved samples per channel to <paramref name="output"/>. Returns false when the frame is broken or does not
    /// end where its length says; a broken frame writes silence and clears the overlap.
    /// </summary>
    [HotPath]
    public bool Decode(ReadOnlySpan<byte> frame, int lengthBits, Span<float> output)
    {
        var reader = new XmaBitReader(frame, 0, lengthBits);
        reader.Advance(XmaLayout.FrameLengthBits);

        var parsed = ReadHeader(ref reader) && DecodeSubframes(ref reader) && !reader.Overrun;

        if (!parsed)
        {
            Reset();
            output[..(XmaLayout.FrameSamples * _channelCount)].Clear();

            return false;
        }

        EmitFrame(output);

        return reader.Position == lengthBits - TrailerBits;
    }

    /// <summary>Forgets the overlap and scale factors, as after a seek or a lost packet.</summary>
    public void Reset()
    {
        foreach (var channel in _channels)
        {
            channel.Reset();
        }
    }

    /// <summary>Takes over the overlap and channel state <paramref name="previous"/> (same channel count) left after its last frame.</summary>
    public void ContinueFrom(XmaFrameDecoder previous)
    {
        for (var channel = 0; channel < _channelCount; channel++)
        {
            _channels[channel].CopyFrom(previous._channels[channel]);
        }
    }

    /// <summary>Writes the half-transform tail of the last frame (256 samples per channel, interleaved).</summary>
    public void Flush(Span<float> output)
    {
        for (var index = 0; index < XmaChannel.FrameStart; index++)
        {
            for (var channel = 0; channel < _channelCount; channel++)
            {
                output[(index * _channelCount) + channel] = _channels[channel].Samples[index];
            }
        }
    }

    [HotPath]
    private bool ReadHeader(ref XmaBitReader reader)
    {
        if (!XmaTiling.Read(ref reader, _channels, _channelCount))
        {
            return false;
        }

        if (_channelCount > 1 && reader.ReadBit() && reader.ReadBit())
        {
            reader.Advance(PostTransformBits * _channelCount * _channelCount);
        }

        reader.Advance(XmaLayout.DynamicRangeBits);
        ReadTrim(ref reader);

        foreach (var channel in _channels)
        {
            channel.DecodedSamples = 0;
            channel.CurrentSubframe = 0;
            channel.HasScaleFactors = false;
        }

        return true;
    }

    [HotPath]
    private void ReadTrim(ref XmaBitReader reader)
    {
        if (!reader.ReadBit())
        {
            TrimStart = 0;
            TrimEnd = 0;

            return;
        }

        if (reader.ReadBit())
        {
            TrimStart = (int)reader.Read(XmaLayout.TrimBits);
        }

        if (reader.ReadBit())
        {
            TrimEnd = (int)reader.Read(XmaLayout.TrimBits);
        }
    }

    [HotPath]
    private bool DecodeSubframes(ref XmaBitReader reader)
    {
        var done = false;

        while (!done)
        {
            if (!_subframes.Decode(ref reader, _channelCount, out done) || reader.Overrun)
            {
                return false;
            }
        }

        return true;
    }

    /// <summary>Writes [0, 512) of every channel and moves the overlap tail [512, 768) to the front for the next frame.</summary>
    [HotPath]
    private void EmitFrame(Span<float> output)
    {
        for (var channel = 0; channel < _channelCount; channel++)
        {
            var samples = _channels[channel].Samples;

            for (var index = 0; index < XmaLayout.FrameSamples; index++)
            {
                output[(index * _channelCount) + channel] = samples[index];
            }

            samples.AsSpan(XmaLayout.FrameSamples, XmaChannel.FrameStart).CopyTo(samples);
        }
    }
}
