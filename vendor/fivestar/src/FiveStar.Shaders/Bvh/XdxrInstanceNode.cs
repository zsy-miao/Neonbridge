// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Buffers.Binary;
using System.Numerics;

namespace FiveStar.Shaders.Bvh;

/// <summary>
/// The 128-byte instance node (type 6) of a console TLAS. The fields from 0x30 on are as the game's traversal reads
/// them (CS_TraceShadowRays_Half, docs/raytracing.md): 0x30 InstanceID | mask &lt;&lt; 24, 0x34 hit-group contribution
/// | flags &lt;&lt; 24, 0x38 the BLAS root (bits 29..0) with bit 31 for a translation-only transform, 0x3C the BLAS
/// address &gt;&gt; 8, 0x40 the world-to-object translation, 0x4C its 3x3 part row-major. 0x00 holds the desc's
/// object-to-world matrix and 0x70 the instance index (not read by that shader; after Leonida's notes).
/// </summary>
public static class XdxrInstanceNode
{
    public const int Size = 128;

    /// <summary>The node type of an instance.</summary>
    public const uint Type = 6;

    private const int MatrixFloats = 12;
    private const int Rows = 3;
    private const int Columns = 4;
    private const int FloatSize = 4;

    private const int IdOffset = 0x30;
    private const int ContributionOffset = 0x34;
    private const int RootOffset = 0x38;
    private const int BlasOffset = 0x3C;
    private const int TranslationOffset = 0x40;
    private const int RotationOffset = 0x4C;
    private const int IndexOffset = 0x70;

    private const uint LowBits = 0xFFFFFF;
    private const int TopByteShift = 24;
    private const uint RootMask = 0x3FFFFFFF;
    private const uint TranslationOnlyBit = 0x80000000;
    private const int BlasShift = 8;

    /// <summary>Writes the node of <paramref name="instance"/>; false when its transform cannot be inverted.</summary>
    public static bool Write(Span<byte> node, BvhInstance instance, uint index)
    {
        var rotation = Rotation(instance.ObjectToWorld);

        if (!Matrix4x4.Invert(rotation, out var inverse))
        {
            return false;
        }

        node[..Size].Clear();

        for (int i = 0; i < MatrixFloats; i++)
        {
            BinaryPrimitives.WriteSingleLittleEndian(node[(FloatSize * i)..], instance.ObjectToWorld[i]);
        }

        uint root = instance.BlasRoot & RootMask;

        if (rotation.IsIdentity)
        {
            root |= TranslationOnlyBit;
        }

        Write(node, IdOffset, (instance.Id & LowBits) | ((uint)instance.Mask << TopByteShift));
        Write(node, ContributionOffset, (instance.Contribution & LowBits) | ((uint)instance.Flags << TopByteShift));
        Write(node, RootOffset, root);
        Write(node, BlasOffset, (uint)(instance.BlasAddress >> BlasShift));
        WriteWorldToObject(node, inverse, Translation(instance.ObjectToWorld));
        Write(node, IndexOffset, index);

        return true;
    }

    /// <summary>The world-space bounds of the BLAS's bounds under the instance transform.</summary>
    public static BvhBounds WorldBounds(BvhInstance instance)
    {
        var local = instance.BlasBounds;
        var rotation = Rotation(instance.ObjectToWorld);
        var translation = Translation(instance.ObjectToWorld);
        var bounds = BvhBounds.Empty;

        for (int corner = 0; corner < (1 << Rows); corner++)
        {
            var high = Vector3.Zero;

            for (int axis = 0; axis < Rows; axis++)
            {
                high[axis] = (corner >> axis) & 1;
            }

            var point = local.Min + ((local.Max - local.Min) * high);
            var world = Apply(rotation, point) + translation;
            bounds = bounds.Union(new BvhBounds(world, world));
        }

        return bounds;
    }

    /// <summary>The 3x3 part as Matrix4x4 rows 1 to 3 (Mij = row i, column j), the rest identity.</summary>
    private static Matrix4x4 Rotation(float[] m)
    {
        var rotation = Matrix4x4.Identity;

        for (int row = 0; row < Rows; row++)
        {
            for (int column = 0; column < Rows; column++)
            {
                rotation[row, column] = m[(row * Columns) + column];
            }
        }

        return rotation;
    }

    /// <summary>Column 3 of the 3x4 matrix.</summary>
    private static Vector3 Translation(float[] m)
    {
        var translation = Vector3.Zero;

        for (int row = 0; row < Rows; row++)
        {
            translation[row] = m[(row * Columns) + Rows];
        }

        return translation;
    }

    /// <summary>The 3x3 matrix in <paramref name="m"/> times the column vector <paramref name="v"/>.</summary>
    private static Vector3 Apply(Matrix4x4 m, Vector3 v)
    {
        return new Vector3(
            (m.M11 * v.X) + (m.M12 * v.Y) + (m.M13 * v.Z),
            (m.M21 * v.X) + (m.M22 * v.Y) + (m.M23 * v.Z),
            (m.M31 * v.X) + (m.M32 * v.Y) + (m.M33 * v.Z));
    }

    /// <summary>World to object: x' = R^-1 x - R^-1 t, the translation first, then R^-1 row-major.</summary>
    private static void WriteWorldToObject(Span<byte> node, Matrix4x4 inverse, Vector3 translation)
    {
        var moved = -Apply(inverse, translation);

        for (int row = 0; row < Rows; row++)
        {
            WriteFloat(node, TranslationOffset + (FloatSize * row), moved[row]);

            for (int column = 0; column < Rows; column++)
            {
                WriteFloat(node, RotationOffset + (FloatSize * ((row * Rows) + column)), inverse[row, column]);
            }
        }
    }

    private static void Write(Span<byte> node, int offset, uint value)
    {
        BinaryPrimitives.WriteUInt32LittleEndian(node[offset..], value);
    }

    private static void WriteFloat(Span<byte> node, int offset, float value)
    {
        BinaryPrimitives.WriteSingleLittleEndian(node[offset..], value);
    }
}
