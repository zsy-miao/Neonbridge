// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Formats.Audio.Xma;

/// <summary>
/// Decodes one XMA2 stream (one or two channels) packet by packet into 32-bit float PCM, full scale 1.0. Frames run on
/// across packet boundaries; the packet header says how many bits of a new packet finish the frame begun before it.
/// Every frame gives 512 samples per channel. The output starts with the decoder's own delay (see docs/xma.md), which
/// <see cref="XmaTrackDecoder"/> trims for whole sounds.
/// </summary>
public sealed class XmaStreamDecoder
{
    /// <summary>Most frames one packet can finish: the 6-bit frame count, plus the one begun in the packet before.</summary>
    public const int MaxFramesPerPacket = 1 << XmaLayout.FrameCountBits;

    private const int BitsPerByte = 8;

    private readonly XmaFrameDecoder _frames;
    private readonly XmaFrameAssembler _assembler = new();

    public XmaStreamDecoder(int channels, int sampleRate)
    {
        _frames = new XmaFrameDecoder(channels, sampleRate);
    }

    public int Channels => _frames.Channels;

    /// <summary>Gets the floats one packet can produce at most: <see cref="MaxFramesPerPacket"/> frames of all channels.</summary>
    public int MaxPacketOutput => MaxFramesPerPacket * XmaLayout.FrameSamples * Channels;

    /// <summary>Gets the frames decoded so far.</summary>
    public long Frames { get; private set; }

    /// <summary>Gets the frames that did not decode cleanly: a broken bitstream, or not ending where their length says.</summary>
    public long BadFrames { get; private set; }

    /// <summary>Gets the partial frames dropped because the next packet did not continue them.</summary>
    public long LostFrames { get; private set; }

    /// <summary>Gets the start trim of the last frame that carried one, in samples.</summary>
    public int TrimStart => _frames.TrimStart;

    /// <summary>Gets the end trim of the last frame that carried one, in samples.</summary>
    public int TrimEnd => _frames.TrimEnd;

    /// <summary>
    /// Decodes one packet and writes the frames it completes, 512 interleaved samples per channel each, to
    /// <paramref name="output"/> (at least <see cref="MaxPacketOutput"/> floats). Returns the frames written.
    /// </summary>
    [HotPath]
    public int DecodePacket(ReadOnlySpan<byte> packet, Span<float> output)
    {
        RequirePacket(packet);

        var header = XmaPacketHeader.Read(packet);
        var payloadBits = (XmaLayout.PacketBytes * BitsPerByte) - XmaLayout.PacketHeaderBits;
        var reader = new XmaBitReader(packet, XmaLayout.PacketHeaderBits, payloadBits);
        var written = FinishCarriedFrame(ref reader, header.FirstFrameOffset, output);

        if (header.FirstFrameOffset >= payloadBits)
        {
            return written;
        }

        return written + ReadFrames(ref reader, output[(written * FrameFloats)..]);
    }

    /// <summary>
    /// Decodes the frame that starts at bit <paramref name="position"/> of <paramref name="buffer"/>, a run of whole
    /// packets of this stream, into <paramref name="output"/> (512 interleaved samples per channel). Positions count
    /// the packet headers, as the console's XMA context read offset does. A frame may run on into the next packet.
    /// Returns false when no whole frame starts there; <paramref name="next"/> is where the frame after it starts, or
    /// null when the buffer ends first. The overlap carries over from the frame decoded before, wherever it was.
    /// </summary>
    [HotPath]
    public bool DecodeFrameAt(ReadOnlySpan<byte> buffer, int position, Span<float> output, out int? next)
    {
        next = null;
        var packet = position / XmaLayout.PacketBits;

        if (position < 0 || packet >= buffer.Length / XmaLayout.PacketBytes || position % XmaLayout.PacketBits < XmaLayout.PacketHeaderBits)
        {
            return false;
        }

        _assembler.Clear();
        var end = position;

        if (!CollectFrom(buffer, ref packet, ref end, XmaLayout.FrameLengthBits))
        {
            return false;
        }

        var length = _assembler.DeclaredLength;

        if (length == XmaLayout.NoFrame || length <= XmaLayout.FrameLengthBits || !CollectFrom(buffer, ref packet, ref end, length - XmaLayout.FrameLengthBits))
        {
            _assembler.Clear();

            return false;
        }

        var more = _assembler.MoreFrames();
        _ = DecodeAssembled(output);
        next = FrameAfter(buffer, packet, end, more);

        return true;
    }

    /// <summary>Writes the tail of the last frame's overlap, 256 interleaved samples per channel, and returns that count.</summary>
    public int Flush(Span<float> output)
    {
        _frames.Flush(output);

        return XmaChannel.FrameStart;
    }

    /// <summary>Starts over, as at the stream's first packet.</summary>
    public void Reset()
    {
        _assembler.Clear();
        _frames.Reset();
    }

    /// <summary>
    /// Starts at a frame boundary as the continuation of <paramref name="previous"/>, a decoder of the same stream format:
    /// the next frame overlaps the last frame <paramref name="previous"/> decoded, as in one continuous decode.
    /// </summary>
    public void ContinueFrom(XmaStreamDecoder previous)
    {
        if (previous.Channels != Channels)
        {
            throw new ArgumentException("the decoders have different channel counts", nameof(previous));
        }

        _assembler.Clear();
        _frames.ContinueFrom(previous._frames);
    }

    private int FrameFloats => XmaLayout.FrameSamples * Channels;

    /// <summary>Adds the first bits of the packet to the frame carried over from the last one and decodes it when whole.</summary>
    [HotPath]
    private int FinishCarriedFrame(ref XmaBitReader reader, int carried, Span<float> output)
    {
        var take = carried < reader.Remaining ? carried : reader.Remaining;

        if (_assembler.Bits == 0 || take == 0)
        {
            DropPartial();
            reader.Advance(take);

            return 0;
        }

        _assembler.Collect(ref reader, take);

        if (_assembler.Complete)
        {
            return DecodeAssembled(output);
        }

        if (take < carried)
        {
            return 0;
        }

        DropPartial();

        return 0;
    }

    /// <summary>Decodes the frames that start in this packet; a last frame that runs on is kept for the next packet.</summary>
    [HotPath]
    private int ReadFrames(ref XmaBitReader reader, Span<float> output)
    {
        var written = 0;

        while (reader.Remaining > 0 && (written + 1) * FrameFloats <= output.Length)
        {
            if (reader.Remaining >= XmaLayout.FrameLengthBits)
            {
                var length = (int)reader.Peek(XmaLayout.FrameLengthBits);

                if (length == XmaLayout.NoFrame || length <= XmaLayout.FrameLengthBits)
                {
                    break;
                }

                if (length <= reader.Remaining)
                {
                    _assembler.Clear();
                    _assembler.Collect(ref reader, length);
                    var more = _assembler.MoreFrames();
                    written += DecodeAssembled(output[(written * FrameFloats)..]);

                    if (!more)
                    {
                        break;
                    }

                    continue;
                }
            }

            _assembler.Clear();
            _assembler.Collect(ref reader, reader.Remaining);
        }

        return written;
    }

    [HotPath]
    private int DecodeAssembled(Span<float> output)
    {
        if (!_frames.Decode(_assembler.Data, _assembler.DeclaredLength, output))
        {
            BadFrames++;
        }

        Frames++;
        _assembler.Clear();

        return 1;
    }

    /// <summary>Collects <paramref name="count"/> bits from <paramref name="position"/> on, past packet headers; false when the buffer ends first.</summary>
    [HotPath]
    private bool CollectFrom(ReadOnlySpan<byte> buffer, ref int packet, ref int position, int count)
    {
        while (count > 0)
        {
            if (position >= (packet + 1) * XmaLayout.PacketBits)
            {
                var following = FollowingPacket(buffer, packet);

                if (following is null)
                {
                    return false;
                }

                packet = following.Value;
                position = (packet * XmaLayout.PacketBits) + XmaLayout.PacketHeaderBits;
            }

            var room = ((packet + 1) * XmaLayout.PacketBits) - position;
            var take = count < room ? count : room;
            var reader = new XmaBitReader(buffer, position, take);
            _assembler.Collect(ref reader, take);
            position += take;
            count -= take;
        }

        return true;
    }

    /// <summary>Where the frame after one that ended at <paramref name="end"/> of <paramref name="packet"/> starts.</summary>
    [HotPath]
    private static int? FrameAfter(ReadOnlySpan<byte> buffer, int packet, int end, bool more)
    {
        var packetEnd = (packet + 1) * XmaLayout.PacketBits;

        if (more && end < packetEnd)
        {
            if (packetEnd - end < XmaLayout.FrameLengthBits)
            {
                return end;
            }

            var length = (int)new XmaBitReader(buffer, end, XmaLayout.FrameLengthBits).Peek(XmaLayout.FrameLengthBits);

            if (length != XmaLayout.NoFrame && length > XmaLayout.FrameLengthBits)
            {
                return end;
            }
        }

        var following = FollowingPacket(buffer, packet);

        if (following is null)
        {
            return null;
        }

        var first = XmaLayout.PacketHeaderBits + PacketAt(buffer, following.Value).FirstFrameOffset;

        if (first >= XmaLayout.PacketBits)
        {
            return null;
        }

        return (following.Value * XmaLayout.PacketBits) + first;
    }

    /// <summary>The stream's packet after <paramref name="packet"/>: past the packets of other streams its header skips.</summary>
    [HotPath]
    private static int? FollowingPacket(ReadOnlySpan<byte> buffer, int packet)
    {
        var following = packet + 1 + PacketAt(buffer, packet).SkipCount;

        if (following >= buffer.Length / XmaLayout.PacketBytes)
        {
            return null;
        }

        return following;
    }

    [HotPath]
    private static XmaPacketHeader PacketAt(ReadOnlySpan<byte> buffer, int packet) =>
        XmaPacketHeader.Read(buffer.Slice(packet * XmaLayout.PacketBytes, XmaLayout.PacketBytes));

    private static void RequirePacket(ReadOnlySpan<byte> packet)
    {
        if (packet.Length < XmaLayout.PacketBytes)
        {
            throw new ArgumentException("an XMA packet is 2048 bytes", nameof(packet));
        }
    }

    private void DropPartial()
    {
        if (_assembler.Bits > 0)
        {
            LostFrames++;
            _assembler.Clear();
        }
    }
}
