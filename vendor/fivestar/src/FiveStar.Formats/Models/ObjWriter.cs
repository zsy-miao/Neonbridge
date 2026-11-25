// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;

namespace FiveStar.Formats.Models;

/// <summary>
/// Writes geometries as Wavefront OBJ: one object per geometry, positions as stored (z up), texture coordinates with
/// v flipped to OBJ's bottom-up convention, and a comment naming each shader's diffuse texture hash. Every vertex gets
/// a texture coordinate and a normal (zero when the geometry has none), so one index serves all three.
/// </summary>
public static class ObjWriter
{
    private const int Components = 3;
    private const int Pair = 2;

    public static void Write(TextWriter writer, Drawable drawable, IReadOnlyList<MeshGeometry> geometries)
    {
        writer.WriteLine($"# {drawable.Name}");

        for (var shader = 0; shader < drawable.Shaders.Count; shader++)
        {
            var diffuse = drawable.Shaders[shader].Diffuse;

            if (diffuse.HasValue)
            {
                writer.WriteLine($"# shader {shader} DiffuseTex {diffuse.Value:x16}");
            }
        }

        var first = 1;

        foreach (var geometry in geometries)
        {
            WriteGeometry(writer, geometry, first);
            first += geometry.VertexCount;
        }
    }

    private static void WriteGeometry(TextWriter writer, MeshGeometry geometry, int first)
    {
        writer.WriteLine($"o geometry_{geometry.Offset:x}");
        writer.WriteLine($"usemtl shader{geometry.Shader}");
        var texCoords = geometry.TexCoords ?? new float[geometry.VertexCount * Pair];
        var normals = geometry.Normals ?? new float[geometry.VertexCount * Components];

        for (var vertex = 0; vertex < geometry.VertexCount; vertex++)
        {
            writer.WriteLine($"v {Triple(geometry.Positions, vertex)}");
            writer.WriteLine($"vt {Number(texCoords[vertex * Pair])} {Number(1 - texCoords[(vertex * Pair) + 1])}");
            writer.WriteLine($"vn {Triple(normals, vertex)}");
        }

        for (var corner = 0; corner < geometry.Indices.Length; corner += Components)
        {
            var a = geometry.Indices[corner] + first;
            var b = geometry.Indices[corner + 1] + first;
            var c = geometry.Indices[corner + Pair] + first;
            writer.WriteLine($"f {a}/{a}/{a} {b}/{b}/{b} {c}/{c}/{c}");
        }
    }

    private static string Triple(float[] values, int vertex)
    {
        var start = vertex * Components;

        return $"{Number(values[start])} {Number(values[start + 1])} {Number(values[start + Pair])}";
    }

    private static string Number(float value)
    {
        return value.ToString("0.######", CultureInfo.InvariantCulture);
    }
}
