// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Buffers.Binary;
using System.Numerics;
using System.Runtime.CompilerServices;
using FiveStar.Loader.Shims.Apu.Dsp;

namespace FiveStar.Loader.Shims.Apu;

/// <summary>
/// One activated XDsp stream on the CPU. A convolution stream convolves each command's input page with the impulse
/// response the activation named and writes the output page; it replaces the signal, the game mixes the dry signal back
/// itself (0x144e5a5ee). The page holds the channels one after the other, <see cref="BlockFrames"/> floats each. A stream
/// that does not convolve (kind 2, or convolution switched off) writes a silent output page.
/// </summary>
internal sealed unsafe class XDspStream
{
    private readonly PartitionedConvolver[] _channels;

    private XDspStream(XDspEffectKind kind, PartitionedConvolver[] channels, int blockFrames)
    {
        Kind = kind;
        _channels = channels;
        BlockFrames = blockFrames;
    }

    public XDspEffectKind Kind { get; }

    /// <summary>Channels the stream convolves; 0 for a silent stream.</summary>
    public int Channels => _channels.Length;

    /// <summary>Frames per channel of one command.</summary>
    public int BlockFrames { get; }

    /// <summary>Partitions of the impulse response per channel; 0 for a silent stream.</summary>
    public int Partitions
    {
        get
        {
            if (_channels.Length == 0)
            {
                return 0;
            }

            return _channels[0].Partitions;
        }
    }

    /// <summary>
    /// The stream an activation config asks for. With <paramref name="convolve"/> a kind 1 config convolves: the
    /// response is read as 32-bit floats, a two-channel response as its channels one after the other (inferred: the
    /// game passes the asset unchanged and nothing on the CPU reads it).
    /// </summary>
    public static XDspStream Create(ReadOnlySpan<byte> config, bool convolve)
    {
        var kind = (XDspEffectKind)BinaryPrimitives.ReadUInt32LittleEndian(config[XDspLayout.ConfigKindOffset..]);
        int frames = (int)BinaryPrimitives.ReadUInt32LittleEndian(config[XDspLayout.ConfigFramesOffset..]);
        int channels = (int)Math.Clamp(BinaryPrimitives.ReadUInt32LittleEndian(config[XDspLayout.ConfigChannelsOffset..]), 1u, (uint)XDspLayout.MaxChannels);
        uint length = BinaryPrimitives.ReadUInt32LittleEndian(config[XDspLayout.ConfigResponseLengthOffset..]);
        long address = BinaryPrimitives.ReadInt64LittleEndian(config[XDspLayout.ConfigResponseAddressOffset..]);

        int block = channels == 1 ? frames : XDspLayout.StereoBlockFrames;
        bool fits = block > 0 && BitOperations.IsPow2(block) && block * channels * sizeof(float) <= XDspLayout.PageBytes;

        if (!convolve || kind != XDspEffectKind.Convolution || address == 0 || length < channels || !fits)
        {
            return new XDspStream(kind, [], block);
        }

        int perChannel = (int)(length / (uint)channels);
        var response = new ReadOnlySpan<float>((void*)address, perChannel * channels);
        var convolvers = new PartitionedConvolver[channels];

        for (int channel = 0; channel < channels; channel++)
        {
            convolvers[channel] = new PartitionedConvolver(response.Slice(channel * perChannel, perChannel), block);
        }

        return new XDspStream(kind, convolvers, block);
    }

    /// <summary>Runs one command: input page to output page.</summary>
    [HotPath]
    public void Process(byte* command)
    {
        var input = (float*)Unsafe.ReadUnaligned<long>(command + XDspLayout.CommandInputOffset);
        var output = (float*)Unsafe.ReadUnaligned<long>(command + XDspLayout.CommandOutputOffset);

        if (output == null)
        {
            return;
        }

        if (_channels.Length == 0 || input == null)
        {
            new Span<byte>(output, XDspLayout.PageBytes).Clear();

            return;
        }

        float gain = Unsafe.ReadUnaligned<float>(command + XDspLayout.CommandGain0Offset) * Unsafe.ReadUnaligned<float>(command + XDspLayout.CommandGain1Offset);

        for (int channel = 0; channel < _channels.Length; channel++)
        {
            var source = new ReadOnlySpan<float>(input + (channel * BlockFrames), BlockFrames);
            var target = new Span<float>(output + (channel * BlockFrames), BlockFrames);
            _channels[channel].Process(source, target);
            Scale(target, gain);
        }
    }

    [HotPath]
    private static void Scale(Span<float> samples, float gain)
    {
        if (gain == 1f)
        {
            return;
        }

        for (int index = 0; index < samples.Length; index++)
        {
            samples[index] *= gain;
        }
    }
}
