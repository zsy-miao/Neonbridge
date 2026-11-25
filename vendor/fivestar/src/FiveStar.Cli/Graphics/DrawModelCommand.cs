// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Diagnostics;
using System.Globalization;
using System.Numerics;
using FiveStar.Cli.Model;
using FiveStar.Cli.Shader;
using FiveStar.Formats.Common;
using FiveStar.Formats.Compression;
using FiveStar.Formats.Images;
using FiveStar.Formats.Models;
using FiveStar.Formats.Rpf;
using FiveStar.Graphics;
using FiveStar.Graphics.Rendering;
using FiveStar.Shaders.Disassembly;

namespace FiveStar.Cli.Graphics;

/// <summary>
/// <c>fivestar graphics draw-model</c>: decodes one drawable carved from an archive and draws it with the game's
/// translated immediate-mode unlit pass and a depth test, textured with its main shader's <c>DiffuseTex</c> (or a paint
/// colour) and shaded by its normals. Given a rim and a tyre, a vehicle fragment gets them on every wheel.
/// </summary>
internal static class DrawModelCommand
{
    public const string Usage =
        "draw-model <out.png> --archive <file.rpf> --from 0xOFFSET [--name TEXT] [--lod N] [--yaw DEG] [--pitch DEG] [--size N] [--plain] [--paint RRGGBB] [--rim 0xOFFSET --tyre 0xOFFSET] [--dump <folder>] [--programs <fssm folder>] [--adapter <index|name|warp>] [--work <dir>] [--debug-layer]";

    private const int Success = 0;
    private const int Failure = 1;
    private const string DumpVariable = "FIVESTAR_DUMP";
    private const string ProgramsVariable = "FIVESTAR_SHADER_DIR";
    private const string DebugFlag = "debug-layer";
    private const string PlainFlag = "plain";
    private const string Warp = "warp";
    private const int DefaultSize = 768;
    private const int DefaultYaw = 35;
    private const int DefaultPitch = 20;
    private const int Corners = 3;
    private const int White = 0xFFFFFF;

    // The wheels' colours: rims and tyres name only the shared placeholder DiffuseTex, like the body paint.
    private const int RimColour = 0xB8B8B8;
    private const int TyreColour = 0x2A2A2A;

    public static int Run(IReadOnlyList<string> args)
    {
        var arguments = new CommandArguments(args, new HashSet<string> { DebugFlag, PlainFlag });

        if (arguments.Positional.Count != 1)
        {
            throw new ArgumentException(Usage);
        }

        var output = Path.GetFullPath(arguments.Positional[0]);
        var dump = Required(arguments.Option("dump") ?? Environment.GetEnvironmentVariable(DumpVariable), "--dump or " + DumpVariable);
        var programs = Required(arguments.Option("programs") ?? Environment.GetEnvironmentVariable(ProgramsVariable), "--programs or " + ProgramsVariable);
        var work = arguments.Option("work") ?? Path.Combine(Path.GetDirectoryName(output)!, Path.GetFileNameWithoutExtension(output) + "-shaders");
        var archive = Required(arguments.Option("archive"), "--archive");
        var clock = Stopwatch.StartNew();

        var drawable = FindDrawable(archive, ModelReports.HexOption(arguments.Option("from")), arguments.Option("name") ?? string.Empty);
        var lod = arguments.IntegerOption("lod") ?? 0;
        var parts = Body(drawable, lod, dump, arguments.Flag(PlainFlag), Paint(arguments.Option("paint")));
        parts.AddRange(Wheels(drawable, archive, arguments.Option("rim"), arguments.Option("tyre")));
        var pass = UnlitPass.Load(dump, programs, work, new RdnaDecoder(), ShaderTools.Dxc(arguments));

        var size = arguments.IntegerOption("size") ?? DefaultSize;
        var view = new ModelView(size, size, arguments.IntegerOption("yaw") ?? DefaultYaw, arguments.IntegerOption("pitch") ?? DefaultPitch, UnlitRenderer.Depth(pass.Pipeline));
        var batches = view.Batches(parts);

        using var device = Open(arguments.Option("adapter"), arguments.Flag(DebugFlag));
        var image = UnlitRenderer.Draw(device, pass.Pipeline, pass.Vertex, pass.Pixel, batches, size, size);
        PngWriter.Write(output, image.Width, image.Height, image.Pixels);
        Console.WriteLine($"{device.AdapterName}: wrote {output} ({batches.Sum(batch => batch.VertexCount) / Corners} front-facing triangles, {parts.Count} parts) in {clock.Elapsed.TotalSeconds:F1} s");

        foreach (var error in image.Errors)
        {
            Console.Error.WriteLine($"debug layer: {error}");
        }

        if (image.Errors.Count > 0)
        {
            return Failure;
        }

        return Success;
    }

    private static Drawable FindDrawable(string archive, long from, string name)
    {
        using var source = new MappedFileSource(archive);
        var carver = new ZlibEntryCarver(source, new ZlibStreamProbe(source, new ZlibInflater()));

        foreach (var carved in DrawableScanner.Scan(carver.CarveFrom(from, 0)))
        {
            if (carved.Drawable.Name.Contains(name, StringComparison.OrdinalIgnoreCase))
            {
                Console.WriteLine($"{carved.Drawable.Name} at 0x{carved.Offset:x}: {carved.Drawable.Geometries.Count} geometries, {carved.Drawable.Problems.Count} problems, {carved.Drawable.Skeleton.Bones.Count} bones, {carved.Drawable.Wheels.Count} wheel cylinders");

                return carved.Drawable;
            }
        }

        throw new ArgumentException($"no drawable named *{name}* after 0x{from:x}");
    }

    /// <summary>
    /// The drawable's geometries at one level of detail: the main shader's with its diffuse texture when the dump has
    /// it, everything else (or everything, with <paramref name="plain"/>) in the paint colour.
    /// </summary>
    private static List<ModelPart> Body(Drawable drawable, int lod, string dump, bool plain, TextureData paint)
    {
        var geometries = drawable.Lod(lod);

        if (geometries.Count == 0)
        {
            throw new ArgumentException($"{drawable.Name} has no decoded geometry at level of detail {lod}");
        }

        var shader = geometries.GroupBy(geometry => geometry.Shader).MaxBy(group => group.Sum(geometry => geometry.TriangleCount))!.Key;
        var hash = drawable.Shaders.ElementAtOrDefault(shader)?.Diffuse;
        (string Path, int Index)? found = null;

        if (!plain && hash.HasValue)
        {
            found = ModelTexture.Find(dump, hash.Value);
        }

        if (!found.HasValue)
        {
            Console.WriteLine($"{geometries.Count} geometries in the paint colour (main shader {shader}, DiffuseTex {hash:x16}, plain {plain})");

            return [new ModelPart(drawable.Name, geometries, Matrix4x4.Identity, paint)];
        }

        var chosen = geometries.Where(geometry => geometry.Shader == shader).ToList();
        var rest = geometries.Where(geometry => geometry.Shader != shader).ToList();
        Console.WriteLine($"shader {shader}: {chosen.Count} of {geometries.Count} geometries, DiffuseTex {hash:x16} = {found.Value.Path} #{found.Value.Index}");

        return
        [
            new ModelPart(drawable.Name, chosen, Matrix4x4.Identity, ModelTexture.Load(found.Value.Path, found.Value.Index)),
            new ModelPart(drawable.Name + " rest", rest, Matrix4x4.Identity, paint),
        ];
    }

    /// <summary>A rim and a tyre (level of detail 0 of the drawables at those offsets) on every wheel, or nothing.</summary>
    private static IReadOnlyList<ModelPart> Wheels(Drawable vehicle, string archive, string? rim, string? tyre)
    {
        if (rim is null || tyre is null)
        {
            return [];
        }

        if (vehicle.Wheels.Count == 0)
        {
            Console.WriteLine($"{vehicle.Name} has no wheel cylinders; --rim and --tyre are not used");

            return [];
        }

        var rimGeometry = FindDrawable(archive, ModelReports.HexOption(rim), string.Empty).Lod(0);
        var tyreGeometry = FindDrawable(archive, ModelReports.HexOption(tyre), string.Empty).Lod(0);
        var parts = VehicleWheels.Parts(vehicle, rimGeometry, tyreGeometry, ModelTexture.Solid(RimColour), ModelTexture.Solid(TyreColour));

        foreach (var wheel in vehicle.Wheels)
        {
            Console.WriteLine($"wheel cylinder: radius {wheel.Radius:F3} m, width {wheel.Width:F3} m");
        }

        return parts;
    }

    private static TextureData Paint(string? rgb)
    {
        if (rgb is null)
        {
            return ModelTexture.Solid(White);
        }

        return ModelTexture.Solid(int.Parse(rgb, NumberStyles.HexNumber, CultureInfo.InvariantCulture));
    }

    private static GpuDevice Open(string? adapter, bool debugLayer)
    {
        if (string.Equals(adapter, Warp, StringComparison.OrdinalIgnoreCase))
        {
            return GpuDevice.CreateWarp(debugLayer);
        }

        return new GpuDevice(PsoCommand.SelectAdapter(adapter).Index, debugLayer);
    }

    private static string Required(string? value, string source)
    {
        if (string.IsNullOrWhiteSpace(value))
        {
            throw new ArgumentException($"set {source}");
        }

        return value;
    }
}
