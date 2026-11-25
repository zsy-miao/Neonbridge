// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Formats.Models;

/// <summary>
/// Reads the wheel cylinders of a fragment (<c>.#ft</c>). The fragment type is the resource's root object (virtual
/// offset 0): +0x10 physics, +0x18 the drawable. Physics +0x20 is a composite bound: +0x70 pointer array of child
/// bounds, +0x98 u16 child count. A child bound has its type in the byte at +0x10 and its half extents at +0x20.
/// Vehicles carry a pair of cylinders (type 8, axis x) per wheel: the brake disc and the tyre.
/// </summary>
public static class FragmentReader
{
    private const int TypeDrawable = 0x18;
    private const int TypePhysics = 0x10;
    private const int PhysicsComposite = 0x20;
    private const int CompositeChildren = 0x70;
    private const int CompositeCount = 0x98;
    private const int BoundType = 0x10;
    private const int BoundHalfExtents = 0x20;
    private const int HalfExtentY = 4;
    private const byte Cylinder = 8;
    private const float Diameter = 2;

    // A cylinder is a tyre when its radius is more than this share of the largest cylinder (the discs are under half).
    private const float TyreShare = 0.5f;

    /// <summary>The tyre cylinders in bound order (which follows the order of the wheel bones), or none for other drawables.</summary>
    public static IReadOnlyList<WheelBound> Wheels(ResourceView view, int drawableRoot)
    {
        if (view.Follow(TypeDrawable) != drawableRoot)
        {
            return [];
        }

        var physics = view.Follow(TypePhysics);

        if (!physics.HasValue)
        {
            return [];
        }

        var composite = view.Follow(physics.Value + PhysicsComposite);

        if (!composite.HasValue)
        {
            return [];
        }

        var cylinders = Cylinders(view, composite.Value);

        if (cylinders.Count == 0)
        {
            return [];
        }

        var largest = cylinders.Max(cylinder => cylinder.Radius);

        return cylinders.Where(cylinder => cylinder.Radius > largest * TyreShare).ToList();
    }

    private static List<WheelBound> Cylinders(ResourceView view, int composite)
    {
        var cylinders = new List<WheelBound>();
        var children = view.Follow(composite + CompositeChildren);

        if (!children.HasValue)
        {
            return cylinders;
        }

        for (var index = 0; index < view.U16(composite + CompositeCount); index++)
        {
            var child = view.Follow(children.Value + (index * sizeof(ulong)));

            if (!child.HasValue || view.VirtualBytes(child.Value + BoundType, 1)[0] != Cylinder)
            {
                continue;
            }

            var extents = child.Value + BoundHalfExtents;
            cylinders.Add(new WheelBound(view.F32(extents + HalfExtentY), view.F32(extents) * Diameter));
        }

        return cylinders;
    }
}
