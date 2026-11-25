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

/// <summary>Vehicle fragments of the dump: the skeleton places the wheels, the physics bounds size them.</summary>
public sealed class VehicleFragmentTests
{
    private const string Archive = "levels.rpf";

    [DumpFact]
    public void A_muscle_car_has_four_wheel_bones_at_its_corners_and_four_tyre_cylinders()
    {
        var car = Read(0x10EC398010, "dominator9_hi.#ft");
        var skeleton = car.Skeleton;

        Assert.Equal(386, skeleton.Bones.Count);
        Assert.Equal(("chassis", -1), (skeleton.Bones[0].Name, skeleton.Bones[0].Parent));
        Assert.Equal(new Vector3(-0.805f, 1.521f, 0.023f), skeleton.World(skeleton.IndexOf("wheel_lf")).Translation, new Tolerance(0.001f));
        Assert.Equal(new Vector3(0.819f, -1.225f, 0.023f), skeleton.World(skeleton.IndexOf("wheel_rr")).Translation, new Tolerance(0.001f));

        // One cylinder per wheel: 0.345 m radius, 0.265 m wide (the brake-disc cylinders are left out).
        Assert.Equal(4, car.Wheels.Count);
        Assert.All(car.Wheels, wheel => Assert.Equal((0.345, 0.265), (Math.Round(wheel.Radius, 3), Math.Round(wheel.Width, 3))));
    }

    [DumpFact]
    public void A_sports_car_lists_its_larger_rear_tyres_in_the_order_of_its_wheel_bones()
    {
        var car = Read(0x1135DFB010, "carbonizzare_hi.#ft");
        var wheels = car.Skeleton.Bones.Where(bone => bone.Name is "wheel_lf" or "wheel_rf" or "wheel_lr" or "wheel_rr").Select(bone => bone.Name);

        Assert.Equal(["wheel_lr", "wheel_rr", "wheel_lf", "wheel_rf"], wheels);
        Assert.Equal([0.368, 0.368, 0.345, 0.345], car.Wheels.Select(wheel => Math.Round(wheel.Radius, 3)));
    }

    [DumpFact]
    public void A_muscle_car_is_skinned_to_its_bones_and_its_inverse_binds_undo_the_bone_transforms()
    {
        var car = Read(0x10EC398010, "dominator9_hi.#ft");
        var wheel = car.Skeleton.IndexOf("wheel_lf");

        Assert.True(Matrix4x4.Invert(car.Skeleton.Bones[wheel].InverseBind!.Value, out var bind));
        Assert.Equal(car.Skeleton.World(wheel).Translation, bind.Translation, new Tolerance(0.001f));

        // Joints name bones (u16 on this 386-bone skeleton where needed); weights sum to 255 per vertex.
        var skinned = car.Lod(0).Where(geometry => geometry.Extras.Skin is not null).ToList();
        Assert.Contains(skinned, geometry => geometry.Extras.Skin!.Joints.Any(joint => joint > byte.MaxValue));

        foreach (var geometry in skinned)
        {
            var skin = geometry.Extras.Skin!;

            Assert.All(skin.Joints, joint => Assert.InRange(joint, 0, car.Skeleton.Bones.Count - 1));
            Assert.All(skin.Weights.Chunk(skin.Influences), weights => Assert.Equal(255, weights.Sum(weight => weight)));
        }
    }

    [DumpFact]
    public void An_ambulance_body_of_more_than_65535_meshlet_vertices_decodes_whole()
    {
        // Geometry 19: 542 meshlets, 59,541 triangles over 67,575 remap entries; the u16 meshlet starts wrap.
        var ambulance = Read(0x10B5111010, "ambulance_hi.#ft");
        var body = ambulance.Lod(0).Single(geometry => geometry.MeshletCount == 542);

        Assert.Empty(ambulance.Problems);
        Assert.Equal(59541, body.TriangleCount);
    }

    private static Drawable Read(long offset, string name)
    {
        using var source = new MappedFileSource(Path.Combine(DumpFactAttribute.Folder!, Archive));
        var carver = new ZlibEntryCarver(source, new ZlibStreamProbe(source, new ZlibInflater()));
        var carved = DrawableScanner.Scan(carver.CarveFrom(offset, 0)).First();

        Assert.Equal((offset, name), (carved.Offset, carved.Drawable.Name));

        return carved.Drawable;
    }

    private sealed class Tolerance : IEqualityComparer<Vector3>
    {
        private readonly float _tolerance;

        public Tolerance(float tolerance)
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
