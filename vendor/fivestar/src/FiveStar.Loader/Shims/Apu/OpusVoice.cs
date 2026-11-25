// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using Concentus;

namespace FiveStar.Loader.Shims.Apu;

/// <summary>
/// The decoder state of one XApu Opus stream. Opus packets depend on the packets before them, so a stream keeps one
/// decoder from its first packet to a reset; the decoder is made at the first decode, when the channel count is known.
/// </summary>
internal sealed class OpusVoice
{
    private readonly float[] _scratch = new float[XApuLayout.MaxPacketSamples * XApuLayout.MaxChannels];
    private IOpusDecoder? _decoder;

    /// <summary>
    /// Decodes <paramref name="packet"/>, drops its first <paramref name="first"/> samples per channel and copies at most
    /// <paramref name="keep"/> samples per channel, interleaved, as fit into <paramref name="output"/>; returns the
    /// samples per channel written.
    /// </summary>
    public int Decode(ReadOnlySpan<byte> packet, Span<float> output, int channels, int first, int keep)
    {
        if (_decoder is null || _decoder.NumChannels != channels)
        {
            _decoder = OpusCodecFactory.CreateDecoder(XApuLayout.OpusRate, channels);
        }

        int decoded = _decoder.Decode(packet, _scratch, XApuLayout.MaxPacketSamples, false);
        int skipped = Math.Min(first, decoded);
        int samples = Math.Min(Math.Min(decoded - skipped, keep), output.Length / channels);
        _scratch.AsSpan(skipped * channels, samples * channels).CopyTo(output);

        return samples;
    }

    public void Reset()
    {
        _decoder?.ResetState();
    }
}
