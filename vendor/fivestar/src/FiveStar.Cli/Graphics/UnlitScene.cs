// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Buffers.Binary;

namespace FiveStar.Cli.Graphics;

/// <summary>
/// What the sample draws: the texture once at its own size (one texel per pixel, white vertex colour) and once as a
/// strip below it with a red-to-blue vertex colour at half alpha, on a target with a margin around both. Vertices
/// follow the pass's input layout: float3 position (pixels, y down), RGBA8 colour, float2 texture coordinate.
/// </summary>
public sealed record UnlitScene(int TextureWidth, int TextureHeight)
{
    public const int Margin = 32;
    public const int VertexStride = 24;

    /// <summary>RGBA8 vertex colours as little-endian words (red in the low byte).</summary>
    public const uint White = 0xFFFFFFFF;
    public const uint HalfRed = 0x800000FF;
    public const uint HalfBlue = 0x80FF0000;

    private const int StripDivisor = 4;
    private const int MarginRows = 3;
    private const int QuadCount = 2;
    private const int PositionY = 4;
    private const int PositionZ = 8;
    private const int ColourOffset = 12;
    private const int TexcoordU = 16;
    private const int TexcoordV = 20;

    // Two triangles per quad as (x, y) corners, counter-clockwise on screen (y down): top-left, bottom-left,
    // bottom-right; top-left, bottom-right, top-right. That is the front face under the cache state
    // (FrontCounterClockwise, back faces culled).
    private static readonly (int X, int Y)[] Corners = [(0, 0), (0, 1), (1, 1), (0, 0), (1, 1), (1, 0)];

    public int Width => TextureWidth + Margin + Margin;

    public int Height => TextureHeight + StripHeight + (MarginRows * Margin);

    public int StripHeight => Math.Max(1, TextureHeight / StripDivisor);

    /// <summary>Gets the first pixel row of the strip.</summary>
    public int StripTop => TextureHeight + Margin + Margin;

    public static int VertexCount => QuadCount * Corners.Length;

    public byte[] Vertices()
    {
        var data = new byte[VertexCount * VertexStride];
        Quad(data.AsSpan(), Margin, Margin, TextureWidth, TextureHeight, White, White);
        Quad(data.AsSpan(Corners.Length * VertexStride), Margin, StripTop, TextureWidth, StripHeight, HalfRed, HalfBlue);

        return data;
    }

    /// <summary>A quad with texture coordinates 0..1; the left corners get one colour, the right corners the other.</summary>
    private static void Quad(Span<byte> data, float left, float top, float width, float height, uint leftColour, uint rightColour)
    {
        for (var index = 0; index < Corners.Length; index++)
        {
            var (x, y) = Corners[index];
            var vertex = data.Slice(index * VertexStride, VertexStride);
            var colour = leftColour;

            if (x == 1)
            {
                colour = rightColour;
            }

            BinaryPrimitives.WriteSingleLittleEndian(vertex, left + (x * width));
            BinaryPrimitives.WriteSingleLittleEndian(vertex[PositionY..], top + (y * height));
            BinaryPrimitives.WriteSingleLittleEndian(vertex[PositionZ..], 0);
            BinaryPrimitives.WriteUInt32LittleEndian(vertex[ColourOffset..], colour);
            BinaryPrimitives.WriteSingleLittleEndian(vertex[TexcoordU..], x);
            BinaryPrimitives.WriteSingleLittleEndian(vertex[TexcoordV..], y);
        }
    }
}
