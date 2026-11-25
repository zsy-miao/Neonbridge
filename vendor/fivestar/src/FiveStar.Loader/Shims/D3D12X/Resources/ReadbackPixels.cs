// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Buffers.Binary;

namespace FiveStar.Loader.Shims.D3D12X.Resources;

/// <summary>
/// Read-back texels as 8-bit RGBA with opaque alpha, for PNG captures and summaries: the 8-bit RGBA and BGRA families,
/// R10G10B10A2, R16G16B16A16_FLOAT and R11G11B10_FLOAT (clamped to 0..1), one-channel 8-bit formats, depth (the 32-bit
/// plane) and one-channel floats (as grey). DXGI_FORMAT values from dxgiformat.h.
/// </summary>
internal static class ReadbackPixels
{
    private const int Rgba16Typeless = 9;
    private const int Rgba16Float = 10;
    private const int Rgb10A2Typeless = 23;
    private const int Rgb10A2Uint = 25;
    private const int Rgba8Typeless = 27;
    private const int Rgba8Uint = 30;
    private const int R8Typeless = 60;
    private const int R8Unorm = 61;
    private const int A8Unorm = 65;
    private const int Bgra8Unorm = 87;
    private const int Bgra8SrgbX = 93;
    private const int DepthStencilTypeless = 19;
    private const int DepthStencilFloat = 20;
    private const int R11G11B10Float = 26;
    private const int R32Typeless = 39;
    private const int D32Float = 40;
    private const int R32Float = 41;
    private const int R16Float = 54;

    private const uint ElevenBitMask = 0x7FF;
    private const int GreenElevenShift = 11;
    private const int BlueTenShift = 22;
    private const int MantissaEleven = 6;
    private const int MantissaTen = 5;
    private const int SmallFloatBias = 15;

    private const int Channels = 4;

    /// <summary>Byte offsets of the channels in an RGBA texel.</summary>
    private const int Red = 0;
    private const int Green = 1;
    private const int Blue = 2;
    private const int Alpha = 3;
    private const int Opaque = 255;
    private const float Scale = 255f;
    private const int TenBitMask = 0x3FF;
    private const int TenBitMax = 1023;
    private const int GreenShift = 10;
    private const int BlueShift = 20;
    private const int HalfBytes = 2;

    /// <summary>RGBA8 rows of <paramref name="image"/>, or null for a format this does not read.</summary>
    public static byte[]? ToRgba(ReadbackImage image)
    {
        int format = image.Format;
        var rgba = new byte[image.Width * image.Height * Channels];

        for (int y = 0; y < image.Height; y++)
        {
            var row = image.Pixels.AsSpan(y * image.RowBytes, image.RowBytes);
            var output = rgba.AsSpan(y * image.Width * Channels, image.Width * Channels);

            if (!Row(format, row, output, image.Width))
            {
                return null;
            }
        }

        return rgba;
    }

    /// <summary>Non-black pixel count, their bounding box and the mean colour of an RGBA8 image.</summary>
    public static string Summarize(byte[] rgba, int width, int height)
    {
        var tally = new PixelTally(width, height);

        for (int y = 0; y < height; y++)
        {
            tally.Add(rgba.AsSpan(y * width * Channels, width * Channels), y);
        }

        return tally.Describe();
    }

    /// <summary>
    /// The summary of <see cref="Summarize(byte[], int, int)"/> read straight from texels laid out as the GPU copied them
    /// (<paramref name="rowPitch"/> bytes apart, a mapped readback buffer), one row converted at a time into a pooled
    /// buffer; with a non-empty <paramref name="image"/> (width × height × 4 bytes) the RGBA8 rows also go there, for a
    /// PNG. Null for a format this does not read.
    /// </summary>
    public static string? Summarize(ReadOnlySpan<byte> source, int rowPitch, int width, int height, int format, Span<byte> image)
    {
        int rowBytes = width * Channels;
        byte[] scratch = System.Buffers.ArrayPool<byte>.Shared.Rent(rowBytes);

        try
        {
            var tally = new PixelTally(width, height);

            for (int y = 0; y < height; y++)
            {
                var output = image.IsEmpty ? scratch.AsSpan(0, rowBytes) : image.Slice(y * rowBytes, rowBytes);

                if (!Row(format, source.Slice(y * rowPitch), output, width))
                {
                    return null;
                }

                tally.Add(output, y);
            }

            return tally.Describe();
        }
        finally
        {
            System.Buffers.ArrayPool<byte>.Shared.Return(scratch);
        }
    }

    private static bool Row(int format, ReadOnlySpan<byte> row, Span<byte> output, int width)
    {
        if (format is >= Rgba8Typeless and <= Rgba8Uint)
        {
            Swizzle(row, output, width, Red, Blue);

            return true;
        }

        if (format is >= Bgra8Unorm and <= Bgra8SrgbX)
        {
            Swizzle(row, output, width, Blue, Red);

            return true;
        }

        if (format is >= Rgb10A2Typeless and <= Rgb10A2Uint)
        {
            TenBit(row, output, width);

            return true;
        }

        if (format is Rgba16Typeless or Rgba16Float)
        {
            Half(row, output, width);

            return true;
        }

        if (format is R8Typeless or R8Unorm or A8Unorm)
        {
            Grey(row, output, width);

            return true;
        }

        return MoreFormats(format, row, output, width);
    }

    /// <summary>Depth and the float formats of the frame's targets: depth and one-channel floats as grey, R11G11B10 as colour.</summary>
    private static bool MoreFormats(int format, ReadOnlySpan<byte> row, Span<byte> output, int width)
    {
        if (format is DepthStencilTypeless or DepthStencilFloat or R32Typeless or D32Float or R32Float)
        {
            for (int x = 0; x < width; x++)
            {
                Put(output, x, BinaryPrimitives.ReadSingleLittleEndian(row[(x * sizeof(float))..]));
            }

            return true;
        }

        if (format == R16Float)
        {
            for (int x = 0; x < width; x++)
            {
                Put(output, x, (float)BitConverter.UInt16BitsToHalf(BinaryPrimitives.ReadUInt16LittleEndian(row[(x * HalfBytes)..])));
            }

            return true;
        }

        if (format == R11G11B10Float)
        {
            for (int x = 0; x < width; x++)
            {
                uint texel = BinaryPrimitives.ReadUInt32LittleEndian(row[(x * Channels)..]);
                int at = x * Channels;
                output[at] = ToByte(SmallFloat(texel & ElevenBitMask, MantissaEleven));
                output[at + Green] = ToByte(SmallFloat((texel >> GreenElevenShift) & ElevenBitMask, MantissaEleven));
                output[at + Blue] = ToByte(SmallFloat(texel >> BlueTenShift, MantissaTen));
                output[at + Alpha] = Opaque;
            }

            return true;
        }

        return false;
    }

    /// <summary>An unsigned 11- or 10-bit float (5-bit exponent, bias 15, no sign).</summary>
    private static float SmallFloat(uint bits, int mantissaBits)
    {
        uint exponent = bits >> mantissaBits;
        uint mantissa = bits & ((1u << mantissaBits) - 1);
        float fraction = mantissa / (float)(1u << mantissaBits);

        if (exponent == 0)
        {
            return MathF.ScaleB(fraction, 1 - SmallFloatBias);
        }

        return MathF.ScaleB(1 + fraction, (int)exponent - SmallFloatBias);
    }

    private static byte ToByte(float value)
    {
        return (byte)(Math.Clamp(value, 0f, 1f) * Scale);
    }

    private static void Put(Span<byte> output, int x, float value)
    {
        int at = x * Channels;
        byte grey = ToByte(value);
        output[at] = grey;
        output[at + Green] = grey;
        output[at + Blue] = grey;
        output[at + Alpha] = Opaque;
    }

    private static void Swizzle(ReadOnlySpan<byte> row, Span<byte> output, int width, int redAt, int blueAt)
    {
        for (int x = 0; x < width; x++)
        {
            int at = x * Channels;
            output[at] = row[at + redAt];
            output[at + Green] = row[at + Green];
            output[at + Blue] = row[at + blueAt];
            output[at + Alpha] = Opaque;
        }
    }

    private static void TenBit(ReadOnlySpan<byte> row, Span<byte> output, int width)
    {
        for (int x = 0; x < width; x++)
        {
            uint texel = BinaryPrimitives.ReadUInt32LittleEndian(row[(x * Channels)..]);
            int at = x * Channels;
            output[at] = (byte)((texel & TenBitMask) * Opaque / TenBitMax);
            output[at + Green] = (byte)(((texel >> GreenShift) & TenBitMask) * Opaque / TenBitMax);
            output[at + Blue] = (byte)(((texel >> BlueShift) & TenBitMask) * Opaque / TenBitMax);
            output[at + Alpha] = Opaque;
        }
    }

    private static void Half(ReadOnlySpan<byte> row, Span<byte> output, int width)
    {
        for (int x = 0; x < width; x++)
        {
            for (int channel = 0; channel < Channels - 1; channel++)
            {
                var bits = BinaryPrimitives.ReadUInt16LittleEndian(row[(((x * Channels) + channel) * HalfBytes)..]);
                float value = Math.Clamp((float)BitConverter.UInt16BitsToHalf(bits), 0f, 1f);
                output[(x * Channels) + channel] = (byte)(value * Scale);
            }

            output[(x * Channels) + Channels - 1] = Opaque;
        }
    }

    private static void Grey(ReadOnlySpan<byte> row, Span<byte> output, int width)
    {
        for (int x = 0; x < width; x++)
        {
            int at = x * Channels;
            output[at] = row[x];
            output[at + Green] = row[x];
            output[at + Blue] = row[x];
            output[at + Alpha] = Opaque;
        }
    }

    /// <summary>The running counts of a summary: non-black pixels, their box and the colour sums.</summary>
    private sealed class PixelTally
    {
        private readonly int _width;
        private readonly int _height;
        private long _lit;
        private long _red;
        private long _green;
        private long _blue;
        private int _left;
        private int _top;
        private int _right = -1;
        private int _bottom = -1;

        public PixelTally(int width, int height)
        {
            _width = width;
            _height = height;
            _left = width;
            _top = height;
        }

        public void Add(ReadOnlySpan<byte> row, int y)
        {
            for (int x = 0; x < _width; x++)
            {
                int at = x * Channels;
                _red += row[at];
                _green += row[at + Green];
                _blue += row[at + Blue];

                if ((row[at] | row[at + Green] | row[at + Blue]) == 0)
                {
                    continue;
                }

                _lit++;
                _left = Math.Min(_left, x);
                _right = Math.Max(_right, x);
                _top = Math.Min(_top, y);
                _bottom = Math.Max(_bottom, y);
            }
        }

        public string Describe()
        {
            long count = Math.Max(1L, (long)_width * _height);
            string box = "none";

            if (_lit > 0)
            {
                box = $"({_left},{_top})-({_right},{_bottom})";
            }

            return $"{_lit} of {count} pixels not black, box {box}, mean rgb ({_red / count}, {_green / count}, {_blue / count})";
        }
    }
}
