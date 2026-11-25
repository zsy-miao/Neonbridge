// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Tests.Cases;

/// <summary>
/// A flat-shading rasterizer model for the graphics stage cases: a pixel is covered when its centre lies strictly inside
/// a triangle given in pixel coordinates (the cases place no centre on an edge); later triangles overwrite earlier ones.
/// </summary>
public static class Raster
{
    /// <summary>Signed distance-like edge function of (x, y) against the edge a -> b.</summary>
    public static double Edge((double X, double Y) a, (double X, double Y) b, double x, double y)
    {
        return ((b.X - a.X) * (y - a.Y)) - ((b.Y - a.Y) * (x - a.X));
    }

    /// <summary>Paints every pixel whose centre is inside the triangle with <paramref name="value"/>.</summary>
    public static void Fill<T>(T?[,] image, (double X, double Y)[] corners, T value)
        where T : class
    {
        var area = Edge(corners[0], corners[1], corners[2].X, corners[2].Y);

        if (area == 0)
        {
            return;
        }

        for (var y = 0; y < image.GetLength(0); y++)
        {
            for (var x = 0; x < image.GetLength(1); x++)
            {
                var edges = Enumerable.Range(0, 3).Select(index => Edge(corners[index], corners[(index + 1) % 3], x + 0.5, y + 0.5)).ToArray();

                if (edges.Any(edge => Math.Abs(edge) < 1e-9))
                {
                    throw new InvalidOperationException($"pixel centre ({x}, {y}) lies on a triangle edge");
                }

                if (edges.All(edge => (edge > 0) == (area > 0)))
                {
                    image[y, x] = value;
                }
            }
        }
    }
}
