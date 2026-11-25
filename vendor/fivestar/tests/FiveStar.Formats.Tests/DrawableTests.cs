// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Numerics;
using FiveStar.Formats.Common;
using FiveStar.Formats.Compression;
using FiveStar.Formats.Models;
using FiveStar.Formats.Rpf;
using FiveStar.Formats.Tests.Support;
using Xunit;

namespace FiveStar.Formats.Tests;

/// <summary>Drawable geometry: meshlet and position decoding on hand-made bytes, and real drawables of the dump.</summary>
public sealed class DrawableTests
{
    private const string Archive = @"pack\core_restofworld\core_restofworld.rpf";

    [Fact]
    public void Meshlets_expand_through_the_remap_table_and_packed_positions_land_on_their_axes()
    {
        // Two meshlets (with the header flags vehicles set): one triangle (0,1,2) over remap entries 0-2, one triangle (2,1,0) over entries 3-5.
        var meshlets = new ByteWriter()
            .U32(Meshlets.Flag | 0x6000_0000 | 2).U32(2).U32(6)
            .U32(0).U32(0).U32(0).U32(0)
            .U32(0).U32((1u << 16) | 3).U32(0).U32(0)
            .U32(0 | (1u << 7) | (2u << 14)).U32(2 | (1u << 7) | (0u << 14) | (1u << 21))
            .U16(4).U16(5).U16(6).U16(0).U16(1).U16(2)
            .ToArray();

        Assert.Equal([4, 5, 6, 2, 1, 0], Meshlets.Triangles(meshlets, 7));

        // Axis word 1: field C (16 bits) holds y, A holds z, B holds x; value = minimum + field * step.
        var header = new ByteWriter().U32(F(1)).U32(F(2)).U32(F(3)).U32(1).U32(F(0.5f)).U32(F(0.25f)).U32(F(2)).U32(F(4)).ToArray();
        var vertex = new ByteWriter().U32((10u << 8) | 0x34).U32((7u << 16) | 0x12).ToArray();

        Assert.Equal([2 + (0x1234 * 0.25f), 3 + (7 * 2), 1 + (10 * 0.5f)], PackedPositions.Decode(header, vertex, 1));
    }

    [Fact]
    public void Meshlet_starts_that_wrap_past_65535_are_unwrapped_along_the_meshlets()
    {
        // 600 meshlets of 128 triangles over 3 remap entries each; meshlet 512 starts at triangle 65,536, stored as 0.
        const int count = 600;
        const int size = 128;
        var writer = new ByteWriter().U32(Meshlets.Flag | count).U32(count * size).U32(count * 3);

        for (var meshlet = 0u; meshlet < count; meshlet++)
        {
            writer.U32(0).U32((((meshlet * size) & 0xFFFF) << 16) | (meshlet * 3)).U32(0).U32(0);
        }

        for (var triangle = 0u; triangle < count * size; triangle++)
        {
            writer.U32(0 | (1u << 7) | (2u << 14) | ((triangle / size) << 21));
        }

        for (var entry = 0; entry < count * 3; entry++)
        {
            writer.U16((ushort)(entry / 3));
        }

        var indices = Meshlets.Triangles(writer.ToArray(), count);

        Assert.Equal([599, 599, 599], indices[^3..]);
        Assert.Equal([512, 512, 512], indices[(512 * size * 3)..((512 * size * 3) + 3)]);
    }

    [DumpFact]
    public void A_prop_decodes_three_levels_of_detail_with_normals_that_face_their_triangles()
    {
        var drawable = Read(0x2063010, "cvar_am_toy_dog_01a.#dr");
        var lod0 = drawable.Lod(0).Single();

        Assert.Empty(drawable.Problems);
        Assert.Equal([310, 177, 110], drawable.Geometries.Select(geometry => geometry.VertexCount));
        Assert.Equal((396, 4), (lod0.TriangleCount, lod0.MeshletCount));
        Assert.Equal(0x38ABDD5345A419E6ul, drawable.Shaders[lod0.Shader].Diffuse);
        Assert.InRange(NormalAgreement(lod0), 0.98, 1);

        // The bounds the drawable stores for this geometry.
        var (low, high) = Bounds(lod0);
        Assert.Equal(new Vector3(-0.0483f, 0.0024f, 0.0015f), low, new Vector3Tolerance(0.0002f));
        Assert.Equal(new Vector3(0.0483f, 0.1523f, 0.0601f), high, new Vector3Tolerance(0.0002f));
    }

    [DumpFact]
    public void A_skinned_garment_and_a_poster_decode_with_their_axis_order_and_texture_scale()
    {
        // Axis word 1: the tights stand 1 m tall along z.
        var tights = Read(0xA71010, "basebottomstights_fr1_002_stockings_lw_long_full.#dr").Lod(0)[0];
        var (low, high) = Bounds(tights);

        Assert.InRange(high.Z - low.Z, 1.0, 1.1);
        Assert.InRange(NormalAgreement(tights), 0.95, 1);

        // The poster's quad shows the right half of its two-poster texture: u 0.5..1, v 0..1.
        var poster = Read(0x2879010, "cvar_movie_poster_01a.#dr").Lod(0).Single();
        var u = poster.TexCoords!.Where((_, index) => index % 2 == 0).ToList();
        var v = poster.TexCoords!.Where((_, index) => index % 2 == 1).ToList();

        Assert.Equal((0.5, 1.0, 0.0, 1.0), (Math.Round(u.Min(), 1), Math.Round(u.Max(), 1), Math.Round(v.Min(), 1), Math.Round(v.Max(), 1)));
    }

    private static Drawable Read(long offset, string name)
    {
        using var source = new MappedFileSource(Path.Combine(DumpFactAttribute.Folder!, Archive));
        var carver = new ZlibEntryCarver(source, new ZlibStreamProbe(source, new ZlibInflater()));
        var carved = DrawableScanner.Scan(carver.CarveFrom(offset, 0)).First();

        Assert.Equal((offset, true, name), (carved.Offset, carved.HasPhysical, carved.Drawable.Name));

        return carved.Drawable;
    }

    /// <summary>The mean cosine between each vertex's stored normal and the area-weighted normal of its triangles.</summary>
    private static double NormalAgreement(MeshGeometry geometry)
    {
        var sums = new Vector3[geometry.VertexCount];

        for (var corner = 0; corner < geometry.Indices.Length; corner += 3)
        {
            var (a, b, c) = (geometry.Indices[corner], geometry.Indices[corner + 1], geometry.Indices[corner + 2]);
            var face = Vector3.Cross(Position(geometry.Positions, b) - Position(geometry.Positions, a), Position(geometry.Positions, c) - Position(geometry.Positions, a));
            sums[a] += face;
            sums[b] += face;
            sums[c] += face;
        }

        var used = Enumerable.Range(0, geometry.VertexCount).Where(vertex => sums[vertex].Length() > 0).ToList();

        return used.Average(vertex => Vector3.Dot(Vector3.Normalize(sums[vertex]), Position(geometry.Normals!, vertex)));
    }

    private static (Vector3 Low, Vector3 High) Bounds(MeshGeometry geometry)
    {
        var points = Enumerable.Range(0, geometry.VertexCount).Select(vertex => Position(geometry.Positions, vertex)).ToList();

        return (points.Aggregate(Vector3.Min), points.Aggregate(Vector3.Max));
    }

    private static Vector3 Position(float[] values, int vertex)
    {
        return new Vector3(values[vertex * 3], values[(vertex * 3) + 1], values[(vertex * 3) + 2]);
    }

    private static uint F(float value)
    {
        return BitConverter.SingleToUInt32Bits(value);
    }

    private sealed class Vector3Tolerance : IEqualityComparer<Vector3>
    {
        private readonly float _tolerance;

        public Vector3Tolerance(float tolerance)
        {
            _tolerance = tolerance;
        }

        public bool Equals(Vector3 x, Vector3 y)
        {
            var difference = Vector3.Abs(x - y);

            return difference.X <= _tolerance && difference.Y <= _tolerance && difference.Z <= _tolerance;
        }

        public int GetHashCode(Vector3 obj)
        {
            return 0;
        }
    }
}
