// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;
using System.Numerics;

namespace FiveStar.Formats.Ttpf;

/// <summary>
/// A tile swizzle given as element-address bits, least significant first: bit k of an element's index inside a tile
/// is a bit of its x (<c>x</c>), y (<c>y</c>) or z (<c>z</c>, volumes only) position: the lowest bit of that axis not
/// yet used, or the bit numbered after the letter (<c>y1y0</c> puts y bit 1 below y bit 0). A pattern with a x bits,
/// b y bits and c z bits describes a 2^a by 2^b by 2^c element tile; tiles are laid out in x, then y, then z.
/// </summary>
public sealed class SwizzlePattern
{
    /// <summary>x, y and z; a pattern letter minus <c>'x'</c> is its axis.</summary>
    private const int Axes = 3;

    private const int XAxis = 0;
    private const int YAxis = 1;
    private const int ZAxis = 2;

    private readonly List<(int Axis, int Bit, int End)> _tokens;
    private readonly int[] _x;
    private readonly int[] _y;
    private readonly int[] _z;

    public SwizzlePattern(string bits)
    {
        Bits = bits;
        _tokens = Parse(bits);
        TileWidth = 1 << _tokens.Count(token => token.Axis == XAxis);
        TileHeight = 1 << _tokens.Count(token => token.Axis == YAxis);
        TileDepth = 1 << _tokens.Count(token => token.Axis == ZAxis);

        _x = new int[TileElements];
        _y = new int[TileElements];
        _z = new int[TileElements];
        FillCoordinates();
    }

    public string Bits { get; }

    /// <summary>Gets the tile width in elements.</summary>
    public int TileWidth { get; }

    /// <summary>Gets the tile height in elements.</summary>
    public int TileHeight { get; }

    /// <summary>Gets the tile depth in elements (1 for a 2D pattern).</summary>
    public int TileDepth { get; }

    /// <summary>Gets the element count of one tile.</summary>
    public int TileElements => 1 << _tokens.Count;

    /// <summary>Gets the x position inside its tile of the element at an index.</summary>
    public int ElementX(int index)
    {
        return _x[index];
    }

    /// <summary>Gets the y position inside its tile of the element at an index.</summary>
    public int ElementY(int index)
    {
        return _y[index];
    }

    /// <summary>
    /// The shortest prefix whose tile covers <paramref name="width"/> by <paramref name="height"/> elements, or the
    /// whole pattern. A texture whose mip 0 fits inside one tile is swizzled with that prefix.
    /// </summary>
    public string FitBits(int width, int height)
    {
        Span<int> used = stackalloc int[Axes];
        var end = 0;

        foreach (var token in _tokens)
        {
            if (Covered(used[XAxis]) >= width && Covered(used[YAxis]) >= height)
            {
                return Bits[..end];
            }

            used[token.Axis] |= 1 << token.Bit;
            end = token.End;
        }

        return Bits;
    }

    /// <summary>Bytes of a width x height x depth element image padded to whole tiles.</summary>
    public long PaddedSize(int width, int height, int bytesPerElement, int depth = 1)
    {
        return Tiles(width, TileWidth) * Tiles(height, TileHeight) * Tiles(depth, TileDepth) * TileElements * bytesPerElement;
    }

    /// <summary>Turns tiled elements into row-major elements (slice after slice), cropping the tile padding.</summary>
    public byte[] Unswizzle(ReadOnlySpan<byte> tiled, int width, int height, int bytesPerElement, int depth = 1)
    {
        var columns = Tiles(width, TileWidth);
        var rows = Tiles(height, TileHeight);
        var layers = Tiles(depth, TileDepth);
        var linear = new byte[(long)width * height * depth * bytesPerElement];
        var extent = (width, height, depth);
        long tile = 0;

        for (var layer = 0; layer < layers; layer++)
        {
            for (var row = 0; row < rows; row++)
            {
                for (var column = 0; column < columns; column++)
                {
                    var origin = (column * TileWidth, row * TileHeight, layer * TileDepth);
                    CopyTile(tiled, linear, tile * TileElements * bytesPerElement, origin, extent, bytesPerElement);
                    tile++;
                }
            }
        }

        return linear;
    }

    private static long Tiles(int extent, int tile)
    {
        return (extent + tile - 1) / tile;
    }

    private void CopyTile(ReadOnlySpan<byte> tiled, byte[] linear, long tileStart, (int X, int Y, int Z) origin, (int Width, int Height, int Depth) extent, int bytesPerElement)
    {
        for (var element = 0; element < TileElements; element++)
        {
            var x = origin.X + _x[element];
            var y = origin.Y + _y[element];
            var z = origin.Z + _z[element];

            if (x >= extent.Width || y >= extent.Height || z >= extent.Depth)
            {
                continue;
            }

            var target = ((((long)z * extent.Height) + y) * extent.Width) + x;
            var source = tiled.Slice((int)(tileStart + ((long)element * bytesPerElement)), bytesPerElement);
            source.CopyTo(linear.AsSpan((int)(target * bytesPerElement)));
        }
    }

    /// <summary>Extent covered by the low bits of an axis that are all present.</summary>
    private static int Covered(int usedBits)
    {
        return 1 << BitOperations.TrailingZeroCount(~usedBits);
    }

    private static List<(int Axis, int Bit, int End)> Parse(string bits)
    {
        var tokens = new List<(int Axis, int Bit, int End)>();
        Span<int> used = stackalloc int[Axes];
        var position = 0;

        while (position < bits.Length)
        {
            var axis = bits[position] - 'x';
            position++;
            var start = position;

            while (position < bits.Length && char.IsAsciiDigit(bits[position]))
            {
                position++;
            }

            var bit = BitOperations.TrailingZeroCount(~used[axis]);

            if (position > start)
            {
                bit = int.Parse(bits.AsSpan(start, position - start), CultureInfo.InvariantCulture);
            }

            used[axis] |= 1 << bit;
            tokens.Add((axis, bit, position));
        }

        return tokens;
    }

    private void FillCoordinates()
    {
        Span<int> position = stackalloc int[Axes];

        for (var element = 0; element < _x.Length; element++)
        {
            position.Clear();

            for (var index = 0; index < _tokens.Count; index++)
            {
                var token = _tokens[index];
                position[token.Axis] |= ((element >> index) & 1) << token.Bit;
            }

            _x[element] = position[XAxis];
            _y[element] = position[YAxis];
            _z[element] = position[ZAxis];
        }
    }
}
