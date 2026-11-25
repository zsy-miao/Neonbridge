// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Tests.Cases;

/// <summary>
/// A model of GS_VoxelizeGeneric (read off its disassembly): n = normalize(cross(v1 - v0, v2 - v0)); dominant axis 0/1 =
/// +-x, 2/3 = +-y, 4/5 = +-z (y wins a tie with x, z must be strictly larger); each vertex is projected to (z, y) for x,
/// (x, z) for y, (x, y) for z and clamped to [-1, 1] (scale 1). Outputs: TEXCOORD0.x = axis, TEXCOORD1 = (-1, -1, 1, 1),
/// TEXCOORD2.xy = texcoord (interpolated: w = 1, affine barycentrics of the pixel centre), TEXCOORD3.xyz = n.
/// </summary>
public static class VoxelModel
{
    private const int Size = GeometryStageRunner.Size;

    // Projected components (screen x, screen y) per axis id.
    private static readonly (int X, int Y)[] Projections = [(2, 1), (2, 1), (0, 2), (0, 2), (0, 1), (0, 1)];

    public static void Paint(VoxelPixel?[,] image, double[][] triangle, int primitive, Func<int, int, (double S, double T)> texcoord, double inside)
    {
        var normal = Normal(triangle);
        var axis = DominantAxis(normal);
        var (px, py) = Projections[axis];
        var corners = triangle.Select(v => ((Math.Clamp(v[px], -1, 1) + 1) * (Size / 2.0), (1 - Math.Clamp(v[py], -1, 1)) * (Size / 2.0))).ToArray();
        var area = Raster.Edge(corners[0], corners[1], corners[2].Item1, corners[2].Item2);
        var sign = Math.Sign(area);

        for (var y = 0; y < Size; y++)
        {
            for (var x = 0; x < Size; x++)
            {
                var (cx, cy) = (x + 0.5, y + 0.5);
                var distances = Enumerable.Range(0, 3).Select(i => sign * Raster.Edge(corners[i], corners[(i + 1) % 3], cx, cy) / Length(corners[i], corners[(i + 1) % 3])).ToArray();

                if (distances.All(distance => distance > inside))
                {
                    var weights = Enumerable.Range(0, 3).Select(i => Raster.Edge(corners[(i + 1) % 3], corners[(i + 2) % 3], cx, cy) / area).ToArray();
                    var s = Enumerable.Range(0, 3).Sum(k => weights[k] * texcoord(primitive, k).S);
                    var t = Enumerable.Range(0, 3).Sum(k => weights[k] * texcoord(primitive, k).T);
                    image[y, x] = new VoxelPixel(false, (uint)axis, s, t, normal);
                }
                else if (!distances.Any(distance => distance < -inside) && image[y, x] is null)
                {
                    image[y, x] = VoxelPixel.Edge;
                }
            }
        }
    }

    private static double[] Normal(double[][] triangle)
    {
        var (a, b, c) = (triangle[0], triangle[1], triangle[2]);
        double[] e1 = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        double[] e2 = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
        double[] n = [(e2[2] * e1[1]) - (e2[1] * e1[2]), (e2[0] * e1[2]) - (e2[2] * e1[0]), (e2[1] * e1[0]) - (e2[0] * e1[1])];
        var length = Math.Sqrt(n.Sum(value => value * value));

        return [.. n.Select(value => value / length)];
    }

    private static int DominantAxis(double[] n)
    {
        var axis = n[1] > 0 ? 2 : 3;

        if (!(Math.Abs(n[1]) > Math.Abs(n[0])))
        {
            axis = n[0] > 0 ? 0 : 1;
        }

        var dominantIndex = (axis >> 1) == 1 ? 1 : 0;
        var dominant = n[dominantIndex];

        if (Math.Abs(n[2]) > Math.Abs(dominant))
        {
            axis = n[2] > 0 ? 4 : 5;
        }

        return axis;
    }

    private static double Length((double X, double Y) a, (double X, double Y) b)
    {
        return Math.Sqrt(((b.X - a.X) * (b.X - a.X)) + ((b.Y - a.Y) * (b.Y - a.Y)));
    }
}
