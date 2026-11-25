// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Numerics;

namespace FiveStar.Formats.Models;

/// <summary>
/// Reads the skeleton of a drawable root (+0x18): +0x28 pointer to the bones, +0x30 pointer to one inverse bind matrix
/// per bone (16 floats, row vectors; the w column holds other data), +0x6C u16 bone count. A bone is 0x50
/// bytes: +0x00 rotation quaternion (x, y, z, w), +0x10 translation, +0x20 scale, +0x30 name pointer, +0x44 s16 next
/// sibling, +0x46 s16 parent (-1 for the root). Transforms are relative to the parent.
/// </summary>
public static class SkeletonReader
{
    private const int SkeletonPointer = 0x18;
    private const int BonesPointer = 0x28;
    private const int InverseBindsPointer = 0x30;
    private const int MatrixBytes = 0x40;
    private const int RowBytes = 0x10;
    private const int Row2 = 2;
    private const int Row3 = 3;
    private const int BoneCount = 0x6C;
    private const int BoneBytes = 0x50;
    private const int TranslationOffset = 0x10;
    private const int ScaleOffset = 0x20;
    private const int NamePointer = 0x30;
    private const int ParentOffset = 0x46;
    private const int MaxName = 128;
    private const int VectorY = 4;
    private const int VectorZ = 8;
    private const int VectorW = 12;

    public static Skeleton Read(ResourceView view, int root)
    {
        var skeleton = view.Follow(root + SkeletonPointer);

        if (!skeleton.HasValue)
        {
            return Skeleton.Empty;
        }

        var bones = view.Follow(skeleton.Value + BonesPointer);

        if (!bones.HasValue)
        {
            return Skeleton.Empty;
        }

        var count = view.U16(skeleton.Value + BoneCount);
        var inverseBinds = view.Follow(skeleton.Value + InverseBindsPointer);
        var list = new List<Bone>(count);

        for (var index = 0; index < count; index++)
        {
            Matrix4x4? inverseBind = null;

            if (inverseBinds.HasValue)
            {
                inverseBind = Matrix(view, inverseBinds.Value + (index * MatrixBytes));
            }

            list.Add(ReadBone(view, bones.Value + (index * BoneBytes)) with { InverseBind = inverseBind });
        }

        return new Skeleton(list);
    }

    private static Bone ReadBone(ResourceView view, int bone)
    {
        var name = string.Empty;
        var text = view.Follow(bone + NamePointer);

        if (text.HasValue)
        {
            name = view.Text(text.Value, MaxName) ?? string.Empty;
        }

        var rotation = new Quaternion(view.F32(bone), view.F32(bone + VectorY), view.F32(bone + VectorZ), view.F32(bone + VectorW));
        var parent = (short)view.U16(bone + ParentOffset);

        return new Bone(name, parent, rotation, Vector(view, bone + TranslationOffset), Vector(view, bone + ScaleOffset), null);
    }

    private static Matrix4x4 Matrix(ResourceView view, int at)
    {
        var x = Vector(view, at);
        var y = Vector(view, at + RowBytes);
        var z = Vector(view, at + (Row2 * RowBytes));
        var w = Vector(view, at + (Row3 * RowBytes));

        return new Matrix4x4(x.X, x.Y, x.Z, 0, y.X, y.Y, y.Z, 0, z.X, z.Y, z.Z, 0, w.X, w.Y, w.Z, 1);
    }

    private static Vector3 Vector(ResourceView view, int at)
    {
        return new Vector3(view.F32(at), view.F32(at + VectorY), view.F32(at + VectorZ));
    }
}
