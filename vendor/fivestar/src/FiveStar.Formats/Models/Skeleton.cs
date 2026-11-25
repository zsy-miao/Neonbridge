// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Numerics;

namespace FiveStar.Formats.Models;

/// <summary>The bones of a drawable, parents before children.</summary>
public sealed record Skeleton(IReadOnlyList<Bone> Bones)
{
    public static Skeleton Empty { get; } = new([]);

    /// <summary>The bone's transform in model space: its own transform followed by its parents' (row vectors).</summary>
    public Matrix4x4 World(int index)
    {
        var world = Matrix4x4.Identity;

        for (var bone = index; bone >= 0; bone = Bones[bone].Parent)
        {
            world *= Local(Bones[bone]);
        }

        return world;
    }

    /// <summary>The index of the bone with this name, or -1.</summary>
    public int IndexOf(string name)
    {
        for (var index = 0; index < Bones.Count; index++)
        {
            if (Bones[index].Name == name)
            {
                return index;
            }
        }

        return -1;
    }

    private static Matrix4x4 Local(Bone bone)
    {
        return Matrix4x4.CreateScale(bone.Scale) * Matrix4x4.CreateFromQuaternion(bone.Rotation) * Matrix4x4.CreateTranslation(bone.Translation);
    }
}
