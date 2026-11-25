// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using FiveStar.Formats.Audio.Xma;

namespace FiveStar.Loader.Shims.Apu;

/// <summary>
/// The SHAPE XMA decoder of one context, on the CPU. Once enabled it decodes frame by frame from the context's input
/// read offset into the context's output ring as 16-bit PCM, one 256-byte block (a subframe of a channel) at a time,
/// while the ring has room and input is left; the context fields it moves are the ones the hardware moves
/// (docs/audio-runtime.md). Every frame gives its 512 samples per channel; nothing is trimmed.
/// </summary>
internal sealed unsafe class XmaContextDecoder
{
    private const int SubframeSamples = XmaLayout.MinSubframeSamples;
    private const int Buffers = 2;
    private const float FullScale = 1 << (XmaLayout.BitsPerSample - 1);

    private readonly float[] _frame = new float[XmaLayout.FrameSamples * XmaLayout.MaxChannels];
    private readonly short[] _pending = new short[XmaLayout.FrameSamples * XmaLayout.MaxChannels];
    private XmaStreamDecoder? _decoder;
    private int _sampleRate;
    private int _pendingStart;
    private int _pendingEnd;
    private int _skipSubframes;

    public bool Enabled { get; private set; }

    /// <summary>Counts the enables, so a work buffer tag names one run of the context (<see cref="XmaWorkBufferTag"/>).</summary>
    public uint Generation { get; private set; }

    /// <summary>
    /// Message 5: starts the context with the format, start skip and buffers the game filled in. Afresh, or as the
    /// continuation of <paramref name="previous"/> (the decoder of the context this one is chained to) when it decoded
    /// the same format: then the first frame overlaps the last frame <paramref name="previous"/> decoded.
    /// </summary>
    public void Enable(uint* context, XmaContextDecoder? previous)
    {
        int channels = XmaContextLayout.Stereo.Read(context) != 0 ? XmaLayout.MaxChannels : 1;
        int rate = XmaContextLayout.SampleRates[(int)XmaContextLayout.SampleRate.Read(context)];

        if (_decoder is null || _decoder.Channels != channels || _sampleRate != rate)
        {
            _decoder = new XmaStreamDecoder(channels, rate);
            _sampleRate = rate;
        }
        else
        {
            _decoder.Reset();
        }

        if (previous is not null)
        {
            ContinueFrom(previous, _decoder);
        }

        Generation++;
        _pendingStart = 0;
        _pendingEnd = 0;
        _skipSubframes = (int)XmaContextLayout.SubframeSkip.Read(context);
        XmaContextLayout.SubframeSkip.Write(context, 0);
        Enabled = true;
    }

    /// <summary>Message 7: the context decodes nothing more until it is enabled again.</summary>
    public void Disable()
    {
        Enabled = false;
    }

    /// <summary>
    /// Decodes into <paramref name="output"/> (the context's ring) until it is full or both input buffers are used up.
    /// <paramref name="input0"/> and <paramref name="input1"/> are the input buffers, their packet counts long. Returns
    /// the blocks written, negative (minus one, minus the blocks) when the input ran out.
    /// </summary>
    [HotPath]
    public int Service(uint* context, ReadOnlySpan<byte> input0, ReadOnlySpan<byte> input1, Span<byte> output)
    {
        int written = 0;

        if (!Enabled || _decoder is null)
        {
            return written;
        }

        while (FreeBlocks(context) > 0)
        {
            if (_pendingStart == _pendingEnd && !DecodeNext(context, input0, input1))
            {
                return -1 - written;
            }

            WriteBlock(context, output);
            written++;
        }

        return written;
    }

    /// <summary>Blocks the ring can take: none while the output is not valid (full), all of them when it is empty.</summary>
    [HotPath]
    private static int FreeBlocks(uint* context)
    {
        int blocks = (int)XmaContextLayout.OutputBlocks.Read(context);

        if (blocks == 0 || XmaContextLayout.OutputValid.Read(context) == 0)
        {
            return 0;
        }

        int write = (int)XmaContextLayout.OutputWriteOffset.Read(context);
        int read = (int)XmaContextLayout.OutputReadOffset.Read(context);

        return blocks - (((write - read) % blocks) + blocks) % blocks;
    }

    /// <summary>Writes one block of the pending frame and moves the write offset; a full ring clears the output valid bit first.</summary>
    [HotPath]
    private void WriteBlock(uint* context, Span<byte> output)
    {
        int blocks = (int)XmaContextLayout.OutputBlocks.Read(context);
        int write = (int)XmaContextLayout.OutputWriteOffset.Read(context);
        var samples = _pending.AsSpan(_pendingStart, SubframeSamples);
        MemoryMarshal.AsBytes(samples).CopyTo(output.Slice(write * XmaContextLayout.BlockBytes, XmaContextLayout.BlockBytes));
        _pendingStart += SubframeSamples;

        int next = (write + 1) % blocks;

        if (next == (int)XmaContextLayout.OutputReadOffset.Read(context))
        {
            XmaContextLayout.OutputValid.Write(context, 0);
        }

        XmaContextLayout.OutputWriteOffset.Write(context, (uint)next);
    }

    /// <summary>Decodes the frame at the read offset of the current input buffer, moving to the other buffer when it is used up.</summary>
    [HotPath]
    private bool DecodeNext(uint* context, ReadOnlySpan<byte> input0, ReadOnlySpan<byte> input1)
    {
        for (int attempt = 0; attempt < Buffers; attempt++)
        {
            bool second = XmaContextLayout.CurrentBuffer.Read(context) != 0;

            if (!InputValid(context, second))
            {
                if (!InputValid(context, !second))
                {
                    return false;
                }

                SwapInput(context);

                continue;
            }

            var buffer = second ? input1 : input0;
            int position = (int)XmaContextLayout.InputReadOffset.Read(context);

            if (position < XmaLayout.PacketHeaderBits)
            {
                position = XmaLayout.PacketHeaderBits;
            }

            if (_decoder!.DecodeFrameAt(buffer, position, _frame, out int? next))
            {
                Emit(context, position, next);

                return true;
            }

            SwapInput(context);
        }

        return false;
    }

    /// <summary>Queues the decoded frame as 16-bit PCM, applies the start skip and the loop, and moves the read offset.</summary>
    [HotPath]
    private void Emit(uint* context, int position, int? next)
    {
        int channels = _decoder!.Channels;
        int floats = XmaLayout.FrameSamples * channels;

        for (int index = 0; index < floats; index++)
        {
            _pending[index] = (short)Math.Clamp(MathF.Round(_frame[index] * FullScale), short.MinValue, short.MaxValue);
        }

        _pendingStart = _skipSubframes < XmaLayout.MaxSubframes ? _skipSubframes * SubframeSamples * channels : 0;
        _pendingEnd = floats;
        _skipSubframes = 0;
        uint loops = XmaContextLayout.LoopCount.Read(context);

        if (loops > 0 && position == (int)XmaContextLayout.LoopEnd.Read(context))
        {
            int last = ((int)XmaContextLayout.LoopEndSubframe.Read(context) + 1) * SubframeSamples * channels;

            if (last < _pendingEnd)
            {
                _pendingEnd = last < _pendingStart ? _pendingStart : last;
            }

            _skipSubframes = (int)XmaContextLayout.LoopStartSkip.Read(context);
            next = (int)XmaContextLayout.LoopStart.Read(context);

            if (loops != XmaContextLayout.InfiniteLoops)
            {
                XmaContextLayout.LoopCount.Write(context, loops - 1);
            }
        }

        if (next is null)
        {
            SwapInput(context);

            return;
        }

        XmaContextLayout.InputReadOffset.Write(context, (uint)next.Value);
    }

    /// <summary>Carries the overlap of <paramref name="previous"/> into <paramref name="decoder"/> when both decode the same format.</summary>
    private void ContinueFrom(XmaContextDecoder previous, XmaStreamDecoder decoder)
    {
        var earlier = previous._decoder;

        if (earlier is null || previous._sampleRate != _sampleRate || earlier.Channels != decoder.Channels)
        {
            return;
        }

        decoder.ContinueFrom(earlier);
    }

    private static bool InputValid(uint* context, bool second)
    {
        var field = second ? XmaContextLayout.InputValid1 : XmaContextLayout.InputValid0;

        return field.Read(context) != 0;
    }

    /// <summary>Marks the current input buffer used up and moves to the other one, at its first packet's header end.</summary>
    private static void SwapInput(uint* context)
    {
        bool second = XmaContextLayout.CurrentBuffer.Read(context) != 0;
        var valid = second ? XmaContextLayout.InputValid1 : XmaContextLayout.InputValid0;
        uint other = second ? 0u : 1u;
        valid.Write(context, 0);
        XmaContextLayout.CurrentBuffer.Write(context, other);
        XmaContextLayout.InputReadOffset.Write(context, XmaLayout.PacketHeaderBits);
    }
}
