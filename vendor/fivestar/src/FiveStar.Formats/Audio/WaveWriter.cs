// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Buffers.Binary;

namespace FiveStar.Formats.Audio;

/// <summary>
/// Writes a RIFF WAVE file around raw AWC data (16-bit PCM, 32-bit float, or XMA2: format tag 0x166 with an
/// XMA2WAVEFORMATEX describing one block of 2 KiB packets), or around decoded samples as 16-bit PCM.
/// </summary>
public static class WaveWriter
{
    private const ushort PcmTag = 1;
    private const ushort FloatTag = 3;
    private const ushort Xma2Tag = 0x166;
    private const ushort XmaStreams = 1;
    private const ushort PcmBits = 16;
    private const ushort FloatBits = 32;
    private const int BitsPerByte = 8;
    private const ushort Xma2ExtraSize = 34;
    private const int WaveFormatSize = 18;
    private const int ChunkHeaderSize = 8;
    private const int RiffFormSize = 4;
    private const byte Xma2EncoderVersion = 4;

    /// <summary>Whether the codec has a WAVE form this writer knows.</summary>
    public static bool Supports(AwcCodec codec) => codec == AwcCodec.Pcm16 || codec == AwcCodec.PcmFloat || codec == AwcCodec.Xma2;

    public static void Write(Stream output, AwcFormat format, ReadOnlySpan<byte> data)
    {
        var codec = (AwcCodec)format.Codec;
        var fmt = codec switch
        {
            AwcCodec.Pcm16 => Plain(PcmTag, PcmBits, format),
            AwcCodec.PcmFloat => Plain(FloatTag, FloatBits, format),
            AwcCodec.Xma2 => Xma2(format, data.Length),
            _ => throw new ArgumentException($"no WAVE form for codec {format.Codec}", nameof(format)),
        };

        var riffSize = RiffFormSize + ChunkHeaderSize + fmt.Length + ChunkHeaderSize + data.Length + (data.Length % sizeof(ushort));
        var header = new byte[ChunkHeaderSize + RiffFormSize];
        "RIFF"u8.CopyTo(header);
        BinaryPrimitives.WriteInt32LittleEndian(header.AsSpan(sizeof(uint)), riffSize);
        "WAVE"u8.CopyTo(header.AsSpan(ChunkHeaderSize));

        output.Write(header);
        WriteChunk(output, "fmt "u8, fmt);
        WriteChunk(output, "data"u8, data);
    }

    /// <summary>Writes decoded float samples (full scale 1.0, interleaved) as a 16-bit PCM WAVE file.</summary>
    public static void WritePcm16(Stream output, uint sampleRate, byte channels, ReadOnlySpan<float> samples)
    {
        var data = new byte[samples.Length * sizeof(short)];

        for (var index = 0; index < samples.Length; index++)
        {
            var scaled = MathF.Round(samples[index] * short.MaxValue);
            var clamped = (short)Math.Clamp(scaled, short.MinValue, short.MaxValue);
            BinaryPrimitives.WriteInt16LittleEndian(data.AsSpan(index * sizeof(short)), clamped);
        }

        var format = new AwcFormat((uint)samples.Length, sampleRate, 0, (byte)AwcCodec.Pcm16, channels);
        Write(output, format, data);
    }

    private static byte[] Plain(ushort tag, ushort bits, AwcFormat format)
    {
        var fmt = new byte[WaveFormatSize];
        var blockAlign = (ushort)(bits / BitsPerByte * format.Channels);
        WriteWaveFormat(fmt, tag, format, format.SampleRate * blockAlign, blockAlign, bits, 0);

        return fmt;
    }

    private static byte[] Xma2(AwcFormat format, int dataSize)
    {
        var fmt = new byte[WaveFormatSize + Xma2ExtraSize];
        var bytesPerSecond = (uint)((long)dataSize * format.SampleRate / Math.Max(format.Frames, 1u));
        var blockAlign = (ushort)(PcmBits / BitsPerByte * format.Channels);
        WriteWaveFormat(fmt, Xma2Tag, format, bytesPerSecond, blockAlign, PcmBits, Xma2ExtraSize);

        var extra = fmt.AsSpan(WaveFormatSize);
        BinaryPrimitives.WriteUInt16LittleEndian(extra, XmaStreams);
        BinaryPrimitives.WriteUInt32LittleEndian(extra[(int)Xma2Field.ChannelMask..], 0);
        BinaryPrimitives.WriteUInt32LittleEndian(extra[(int)Xma2Field.SamplesEncoded..], format.Frames);

        BinaryPrimitives.WriteInt32LittleEndian(extra[(int)Xma2Field.BytesPerBlock..], dataSize);
        BinaryPrimitives.WriteUInt32LittleEndian(extra[(int)Xma2Field.PlayLength..], format.Frames);
        extra[(int)Xma2Field.EncoderVersion] = Xma2EncoderVersion;
        BinaryPrimitives.WriteUInt16LittleEndian(extra[(int)Xma2Field.BlockCount..], 1);

        return fmt;
    }

    private static void WriteWaveFormat(Span<byte> fmt, ushort tag, AwcFormat format, uint bytesPerSecond, ushort blockAlign, ushort bits, ushort extraSize)
    {
        BinaryPrimitives.WriteUInt16LittleEndian(fmt, tag);
        BinaryPrimitives.WriteUInt16LittleEndian(fmt[(int)WaveField.Channels..], format.Channels);
        BinaryPrimitives.WriteUInt32LittleEndian(fmt[(int)WaveField.Rate..], format.SampleRate);

        BinaryPrimitives.WriteUInt32LittleEndian(fmt[(int)WaveField.BytesPerSecond..], bytesPerSecond);
        BinaryPrimitives.WriteUInt16LittleEndian(fmt[(int)WaveField.BlockAlign..], blockAlign);
        BinaryPrimitives.WriteUInt16LittleEndian(fmt[(int)WaveField.Bits..], bits);
        BinaryPrimitives.WriteUInt16LittleEndian(fmt[(int)WaveField.ExtraSize..], extraSize);
    }

    private static void WriteChunk(Stream output, ReadOnlySpan<byte> id, ReadOnlySpan<byte> body)
    {
        Span<byte> header = stackalloc byte[ChunkHeaderSize];
        id.CopyTo(header);
        BinaryPrimitives.WriteInt32LittleEndian(header[sizeof(uint)..], body.Length);

        output.Write(header);
        output.Write(body);

        if (body.Length % sizeof(ushort) != 0)
        {
            output.WriteByte(0);
        }
    }

    private enum WaveField
    {
        Channels = 2,
        Rate = 4,
        BytesPerSecond = 8,
        BlockAlign = 12,
        Bits = 14,
        ExtraSize = 16,
    }

    private enum Xma2Field
    {
        ChannelMask = 2,
        SamplesEncoded = 6,
        BytesPerBlock = 10,
        PlayLength = 18,
        EncoderVersion = 31,
        BlockCount = 32,
    }
}
