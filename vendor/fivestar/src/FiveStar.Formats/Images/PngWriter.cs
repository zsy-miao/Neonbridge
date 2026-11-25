// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Buffers.Binary;
using System.IO.Compression;

namespace FiveStar.Formats.Images;

/// <summary>Writes 8-bit RGBA images as PNG: IHDR, one zlib IDAT with filter 0 on every row, IEND.</summary>
public static class PngWriter
{
    private const int BytesPerPixel = 4;
    private const int ChunkFrame = 12;
    private const int HeaderBytes = 13;
    private const byte BitDepth = 8;
    private const byte ColourTypeRgba = 6;
    private const int HeightField = 4;
    private const int DepthField = 8;
    private const int ColourField = 9;

    private static readonly byte[] Signature = [0x89, (byte)'P', (byte)'N', (byte)'G', 0x0D, 0x0A, 0x1A, 0x0A];

    /// <summary>Writes <paramref name="rgba"/> (rows top first, 4 bytes per pixel) to a PNG file.</summary>
    public static void Write(string path, int width, int height, ReadOnlySpan<byte> rgba)
    {
        using var file = File.Create(path);
        Write(file, width, height, rgba);
    }

    public static void Write(Stream output, int width, int height, ReadOnlySpan<byte> rgba)
    {
        var rowBytes = width * BytesPerPixel;

        if (rgba.Length != rowBytes * height)
        {
            throw new ArgumentException($"expected {rowBytes * height} bytes of RGBA, got {rgba.Length}", nameof(rgba));
        }

        var header = new byte[HeaderBytes];
        BinaryPrimitives.WriteInt32BigEndian(header, width);
        BinaryPrimitives.WriteInt32BigEndian(header.AsSpan(HeightField), height);
        header[DepthField] = BitDepth;
        header[ColourField] = ColourTypeRgba;

        output.Write(Signature);
        Chunk(output, "IHDR"u8, header);
        Chunk(output, "IDAT"u8, Compress(rgba, rowBytes, height));
        Chunk(output, "IEND"u8, []);
    }

    private static byte[] Compress(ReadOnlySpan<byte> rgba, int rowBytes, int height)
    {
        using var buffer = new MemoryStream();

        using (var zlib = new ZLibStream(buffer, CompressionLevel.Optimal, leaveOpen: true))
        {
            for (var y = 0; y < height; y++)
            {
                zlib.WriteByte(0);
                zlib.Write(rgba.Slice(y * rowBytes, rowBytes));
            }
        }

        return buffer.ToArray();
    }

    private static void Chunk(Stream output, ReadOnlySpan<byte> type, ReadOnlySpan<byte> data)
    {
        var chunk = new byte[data.Length + ChunkFrame];
        BinaryPrimitives.WriteInt32BigEndian(chunk, data.Length);
        type.CopyTo(chunk.AsSpan(sizeof(int)));
        data.CopyTo(chunk.AsSpan(sizeof(int) + type.Length));
        var crc = Crc32.Compute(chunk.AsSpan(sizeof(int), type.Length + data.Length));
        BinaryPrimitives.WriteUInt32BigEndian(chunk.AsSpan(chunk.Length - sizeof(uint)), crc);

        output.Write(chunk);
    }
}
