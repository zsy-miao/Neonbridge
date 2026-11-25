// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Numerics;
using System.Text.RegularExpressions;
using FiveStar.Formats.Models;
using FiveStar.Graphics.Rendering;

namespace FiveStar.Cli.Graphics;

/// <summary>
/// Puts a rim and a tyre on every wheel of a vehicle fragment. Vehicles carry no wheel geometry: rims (<c>rim_*</c>) and
/// tyres (<c>tyre_*</c>) are shared drawables modelled at a unit size with the axle along x. Each wheel bone
/// (<c>wheel_lf</c>, <c>wheel_rr</c>, ...) takes the tyre cylinder of the same rank (the fragment stores them in bone
/// order); the pair is scaled so the tyre matches the cylinder's radius and width, and mirrored on the right side.
/// </summary>
public static partial class VehicleWheels
{
    private const string RightSide = "wheel_r";
    private const float Half = 0.5f;
    private const int Axes = 3;

    public static IReadOnlyList<ModelPart> Parts(Drawable vehicle, IReadOnlyList<MeshGeometry> rim, IReadOnlyList<MeshGeometry> tyre, TextureData rimTexture, TextureData tyreTexture)
    {
        var bones = WheelBones(vehicle.Skeleton);
        var (tyreRadius, tyreWidth) = Size(tyre);
        var parts = new List<ModelPart>();

        for (var wheel = 0; wheel < Math.Min(bones.Count, vehicle.Wheels.Count); wheel++)
        {
            var bound = vehicle.Wheels[wheel];
            var name = vehicle.Skeleton.Bones[bones[wheel]].Name;
            var side = 1f;

            if (name.StartsWith(RightSide, StringComparison.Ordinal))
            {
                side = -1f;
            }

            var radial = bound.Radius / tyreRadius;
            var scale = Matrix4x4.CreateScale(side * bound.Width / tyreWidth, radial, radial);
            var transform = scale * vehicle.Skeleton.World(bones[wheel]);
            parts.Add(new ModelPart(name + " rim", rim, transform, rimTexture));
            parts.Add(new ModelPart(name + " tyre", tyre, transform, tyreTexture));
        }

        return parts;
    }

    /// <summary>The wheel bones in skeleton order: <c>wheel_</c>, a side letter and a position, nothing after.</summary>
    public static IReadOnlyList<int> WheelBones(Skeleton skeleton)
    {
        var wheels = new List<int>();

        for (var index = 0; index < skeleton.Bones.Count; index++)
        {
            if (WheelBone().IsMatch(skeleton.Bones[index].Name))
            {
                wheels.Add(index);
            }
        }

        return wheels;
    }

    /// <summary>The tyre's outer radius (largest of y and z) and its width along x, in model units.</summary>
    private static (float Radius, float Width) Size(IReadOnlyList<MeshGeometry> tyre)
    {
        var low = new Vector3(float.MaxValue);
        var high = new Vector3(float.MinValue);

        foreach (var geometry in tyre)
        {
            for (var vertex = 0; vertex < geometry.VertexCount; vertex++)
            {
                var point = new Vector3(geometry.Positions.AsSpan(vertex * Axes, Axes));
                low = Vector3.Min(low, point);
                high = Vector3.Max(high, point);
            }
        }

        var size = high - low;

        return (MathF.Max(size.Y, size.Z) * Half, size.X);
    }

    [GeneratedRegex("^wheel_[lr][a-z0-9]+$", RegexOptions.CultureInvariant)]
    private static partial Regex WheelBone();
}
