// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Buffers.Binary;
using System.Numerics;
using FiveStar.Formats.Models;
using FiveStar.Graphics.Pipelines;

namespace FiveStar.Cli.Graphics;

/// <summary>
/// Turns placed geometries into <see cref="UnlitScene"/>-layout vertices for the unlit pass: the model is turned by yaw
/// (about z) and pitch, fitted into the target with an orthographic view, back faces are dropped on the CPU and the rest
/// are wound counter-clockwise on screen (the cache state's front face). z carries the depth between the nearest and
/// farthest point in the given convention, so the depth test sorts the triangles. The vertex colour is a Lambert term
/// from the decoded normals; the texture adds the rest.
/// </summary>
public sealed record ModelView(int Width, int Height, float YawDegrees, float PitchDegrees, DepthConvention Depth)
{
    private const float Fill = 0.9f;
    private const float Ambient = 0.3f;
    private const float Diffuse = 0.7f;
    private const float FullByte = 255;
    private const int Corners = 3;
    private const int Pair = 2;
    private const uint OpaqueAlpha = 0xFF00_0000;
    private const int GreenShift = 8;
    private const int BlueShift = 16;
    private const int PositionY = 4;
    private const int PositionZ = 8;
    private const int ColourOffset = 12;
    private const int TexcoordU = 16;
    private const int TexcoordV = 20;

    // Light from the upper left, towards the viewer (view space: x right, y away from the viewer, z up).
    private static readonly Vector3 Light = Vector3.Normalize(new Vector3(-0.4f, -0.8f, 0.5f));

    /// <summary>One batch per part: the vertex data of its front-facing triangles, with the part's texture.</summary>
    public IReadOnlyList<DrawBatch> Batches(IReadOnlyList<ModelPart> parts)
    {
        var rotation = Rotation();
        var (low, high) = Bounds(parts, rotation);
        var size = high - low;
        var scale = Fill * MathF.Min(Width / MathF.Max(size.X, float.Epsilon), Height / MathF.Max(size.Z, float.Epsilon));
        var frame = new ViewFrame((low + high) / Pair, scale, low.Y, MathF.Max(size.Y, float.Epsilon));

        return parts.Select(part => new DrawBatch(part.Texture, Vertices(part, rotation, frame))).ToList();
    }

    private Matrix4x4 Rotation()
    {
        var yaw = Matrix4x4.CreateRotationZ(float.DegreesToRadians(YawDegrees));
        // A positive pitch tips the top towards the viewer: the camera looks down on the model.
        var pitch = Matrix4x4.CreateRotationX(float.DegreesToRadians(PitchDegrees));

        return yaw * pitch;
    }

    /// <summary>The view-space bounds of every placed vertex.</summary>
    private static (Vector3 Low, Vector3 High) Bounds(IReadOnlyList<ModelPart> parts, Matrix4x4 rotation)
    {
        var low = new Vector3(float.MaxValue);
        var high = new Vector3(float.MinValue);

        foreach (var part in parts)
        {
            var transform = part.Transform * rotation;

            foreach (var geometry in part.Geometries)
            {
                for (var vertex = 0; vertex < geometry.VertexCount; vertex++)
                {
                    var point = Vector3.Transform(Position(geometry.Positions, vertex), transform);
                    low = Vector3.Min(low, point);
                    high = Vector3.Max(high, point);
                }
            }
        }

        return (low, high);
    }

    private byte[] Vertices(ModelPart part, Matrix4x4 rotation, ViewFrame frame)
    {
        var placement = new Placement(part.Transform * rotation, NormalMatrix(part.Transform) * rotation, part.Transform.GetDeterminant() < 0);
        var triangles = new List<ViewVertex[]>();

        foreach (var geometry in part.Geometries)
        {
            Collect(geometry, placement, frame, triangles);
        }

        var data = new byte[triangles.Count * Corners * UnlitScene.VertexStride];

        for (var index = 0; index < triangles.Count; index++)
        {
            Write(data.AsSpan(index * Corners * UnlitScene.VertexStride), triangles[index]);
        }

        return data;
    }

    /// <summary>The matrix that carries normals: the inverse transpose of the transform (its rotation when it has no scale).</summary>
    private static Matrix4x4 NormalMatrix(Matrix4x4 transform)
    {
        if (!Matrix4x4.Invert(transform, out var inverse))
        {
            return transform;
        }

        var normal = Matrix4x4.Transpose(inverse);
        normal.Translation = Vector3.Zero;
        normal.M14 = 0;
        normal.M24 = 0;
        normal.M34 = 0;

        return normal;
    }

    private void Collect(MeshGeometry geometry, Placement placement, ViewFrame frame, List<ViewVertex[]> triangles)
    {
        for (var corner = 0; corner < geometry.Indices.Length; corner += Corners)
        {
            var corners = new ViewVertex[Corners];
            var points = new Vector3[Corners];

            for (var index = 0; index < Corners; index++)
            {
                var vertex = geometry.Indices[corner + index];
                points[index] = Vector3.Transform(Position(geometry.Positions, vertex), placement.Points) - frame.Centre;
                corners[index] = Corner(geometry, vertex, points[index], placement, frame);
            }

            var faceNormal = Vector3.Cross(points[1] - points[0], points[Pair] - points[0]);

            if (placement.Mirrored)
            {
                faceNormal = -faceNormal;
            }

            if (faceNormal.Y < 0)
            {
                triangles.Add(corners);
            }
        }
    }

    private ViewVertex Corner(MeshGeometry geometry, int vertex, Vector3 point, Placement placement, ViewFrame frame)
    {
        var shade = 1f;

        if (geometry.Normals is not null)
        {
            var normal = Vector3.Normalize(Vector3.TransformNormal(Position(geometry.Normals, vertex), placement.Normals));
            shade = Ambient + (Diffuse * MathF.Max(0, Vector3.Dot(normal, Light)));
        }

        var grey = (uint)MathF.Round(shade * FullByte);
        var colour = OpaqueAlpha | grey | (grey << GreenShift) | (grey << BlueShift);
        var texCoord = Vector2.Zero;

        if (geometry.TexCoords is not null)
        {
            texCoord = new Vector2(geometry.TexCoords[vertex * Pair], geometry.TexCoords[(vertex * Pair) + 1]);
        }

        var distance = (point.Y + frame.Centre.Y - frame.Near) / frame.Span;
        var x = (Width / Pair) + (point.X * frame.Scale);
        var y = (Height / Pair) - (point.Z * frame.Scale);

        return new ViewVertex(new Vector3(x, y, Depth.Depth(Math.Clamp(distance, 0, 1))), colour, texCoord);
    }

    private static Vector3 Position(float[] values, int vertex)
    {
        return new Vector3(values[vertex * Corners], values[(vertex * Corners) + 1], values[(vertex * Corners) + Pair]);
    }

    /// <summary>Writes the three corners, swapped if needed so the screen-space cross product is negative (y down).</summary>
    private static void Write(Span<byte> data, ViewVertex[] corners)
    {
        var (a, b, c) = (corners[0], corners[1], corners[Pair]);
        var cross = ((b.Position.X - a.Position.X) * (c.Position.Y - a.Position.Y)) - ((c.Position.X - a.Position.X) * (b.Position.Y - a.Position.Y));

        if (cross > 0)
        {
            (b, c) = (c, b);
        }

        WriteVertex(data, a);
        WriteVertex(data[UnlitScene.VertexStride..], b);
        WriteVertex(data[(Pair * UnlitScene.VertexStride)..], c);
    }

    private static void WriteVertex(Span<byte> vertex, ViewVertex corner)
    {
        BinaryPrimitives.WriteSingleLittleEndian(vertex, corner.Position.X);
        BinaryPrimitives.WriteSingleLittleEndian(vertex[PositionY..], corner.Position.Y);
        BinaryPrimitives.WriteSingleLittleEndian(vertex[PositionZ..], corner.Position.Z);
        BinaryPrimitives.WriteUInt32LittleEndian(vertex[ColourOffset..], corner.Colour);
        BinaryPrimitives.WriteSingleLittleEndian(vertex[TexcoordU..], corner.TexCoord.X);
        BinaryPrimitives.WriteSingleLittleEndian(vertex[TexcoordV..], corner.TexCoord.Y);
    }

    /// <summary>A pixel position with depth in z, the RGBA8 colour and the texture coordinate.</summary>
    private readonly record struct ViewVertex(Vector3 Position, uint Colour, Vector2 TexCoord);

    /// <summary>Where the view sits: the centre of the bounds, pixels per metre, and the nearest view depth and the depth span.</summary>
    private readonly record struct ViewFrame(Vector3 Centre, float Scale, float Near, float Span);

    /// <summary>A part's transforms into view space for points and normals, and whether it mirrors (flips the winding).</summary>
    private readonly record struct Placement(Matrix4x4 Points, Matrix4x4 Normals, bool Mirrored);
}
