// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Formats.Models;

/// <summary>
/// Finds the drawables of a resource and decodes their geometries. A drawable root is an 8-byte aligned object whose
/// pointer at +0xA8 names a text ending in <c>.#dr</c>, <c>.#ft</c> or <c>.#dd</c>. From the root:
/// <list type="table">
/// <item><term>+0x10</term><description>shader group: +0x10 pointer array of shaders, +0x18 u16 count</description></item>
/// <item><term>+0x50 + 8 * lod</term><description>model list of each of 4 levels of detail: +0x00 pointer array, +0x08 u16 count</description></item>
/// <item><term>model</term><description>+0x08 pointer array of geometries, +0x10 u16 count, +0x20 u16 shader index per geometry</description></item>
/// <item><term>shader</term><description>+0x00 u64 name hash, +0x10 pointer array of textures, +0x20 parameter table</description></item>
/// <item><term>parameter table</term><description>+0x01 u8 texture count; from +0x08, per texture: u32 parameter hash, u32 slot * 4</description></item>
/// <item><term>texture</term><description>+0x30 pointer to a reference whose u64 at +0x08 is the texture's name hash</description></item>
/// </list>
/// </summary>
public static class DrawableReader
{
    public const int LevelsOfDetail = 4;

    private const int NamePointer = 0xA8;
    private const int ShaderGroupPointer = 0x10;
    private const int ModelListPointer = 0x50;
    private const int ArrayCount = 0x08;
    private const int GroupArray = 0x10;
    private const int GroupCount = 0x18;
    private const int ModelGeometries = 0x08;
    private const int ModelCount = 0x10;
    private const int ModelShaderIndices = 0x20;
    private const int ShaderTexturesPointer = 0x10;
    private const int ShaderParameters = 0x20;
    private const int ParameterTextureCount = 0x01;
    private const int ParameterEntries = 0x08;
    private const int ParameterEntryBytes = 8;
    private const int SlotUnit = 4;
    private const int TextureReference = 0x30;
    private const int ReferenceHash = 0x08;
    private const int MaxName = 256;
    private const int Alignment = 8;

    private static readonly string[] NameSuffixes = [".#dr", ".#ft", ".#dd"];

    public static IReadOnlyList<Drawable> Read(ResourceView view)
    {
        var drawables = new List<Drawable>();

        for (var root = 0; root + NamePointer + sizeof(ulong) <= view.VirtualLength; root += Alignment)
        {
            var name = Name(view, root);

            if (name is null || !view.Follow(root + ModelListPointer).HasValue)
            {
                continue;
            }

            try
            {
                drawables.Add(ReadRoot(view, root, name));
            }
            catch (ModelFormatException)
            {
                // A name pointer that is not a drawable root: its lists run outside the segment.
            }
        }

        return drawables;
    }

    private static string? Name(ResourceView view, int root)
    {
        var text = view.Follow(root + NamePointer);

        if (!text.HasValue)
        {
            return null;
        }

        var name = view.Text(text.Value, MaxName);

        if (name is null || !NameSuffixes.Any(suffix => name.EndsWith(suffix, StringComparison.Ordinal)))
        {
            return null;
        }

        return name;
    }

    private static Drawable ReadRoot(ResourceView view, int root, string name)
    {
        var geometries = new List<MeshGeometry>();
        var problems = new List<GeometryProblem>();

        for (var lod = 0; lod < LevelsOfDetail; lod++)
        {
            var list = view.Follow(root + ModelListPointer + (lod * sizeof(ulong)));

            if (!list.HasValue)
            {
                continue;
            }

            foreach (var model in Array(view, list.Value, 0, ArrayCount))
            {
                ReadModel(view, model, lod, geometries, problems);
            }
        }

        return new Drawable(name, root, SafeShaders(view, root), geometries, problems, SafeSkeleton(view, root), SafeWheels(view, root));
    }

    // The skeleton and wheels place parts; a drawable whose skeleton or bounds do not read keeps its geometry.
    private static Skeleton SafeSkeleton(ResourceView view, int root)
    {
        try
        {
            return SkeletonReader.Read(view, root);
        }
        catch (ModelFormatException)
        {
            return Skeleton.Empty;
        }
    }

    private static IReadOnlyList<WheelBound> SafeWheels(ResourceView view, int root)
    {
        try
        {
            return FragmentReader.Wheels(view, root);
        }
        catch (ModelFormatException)
        {
            return [];
        }
    }

    // The shaders only name textures; a drawable whose shader group does not read keeps its geometry.
    private static List<ShaderTextures> SafeShaders(ResourceView view, int root)
    {
        try
        {
            return Shaders(view, root);
        }
        catch (ModelFormatException)
        {
            return [];
        }
    }

    private static void ReadModel(ResourceView view, int model, int lod, List<MeshGeometry> geometries, List<GeometryProblem> problems)
    {
        var parts = Array(view, model, ModelGeometries, ModelCount);
        var shaders = view.Follow(model + ModelShaderIndices);

        for (var index = 0; index < parts.Count; index++)
        {
            var shader = 0;

            if (shaders.HasValue)
            {
                shader = view.U16(shaders.Value + (index * sizeof(ushort)));
            }

            try
            {
                geometries.Add(GeometryReader.Read(view, parts[index], lod, shader));
            }
            catch (ModelFormatException exception)
            {
                problems.Add(new GeometryProblem(lod, parts[index], exception.Message));
            }
            catch (ArgumentOutOfRangeException exception)
            {
                // A count or offset that runs past a buffer the checks above did not cover.
                problems.Add(new GeometryProblem(lod, parts[index], "out of range: " + exception.ParamName));
            }
        }
    }

    private static List<ShaderTextures> Shaders(ResourceView view, int root)
    {
        var shaders = new List<ShaderTextures>();
        var group = view.Follow(root + ShaderGroupPointer);

        if (!group.HasValue)
        {
            return shaders;
        }

        foreach (var shader in Array(view, group.Value, GroupArray, GroupCount))
        {
            shaders.Add(new ShaderTextures(view.U64(shader), TextureSlots(view, shader)));
        }

        return shaders;
    }

    private static List<TextureSlot> TextureSlots(ResourceView view, int shader)
    {
        var slots = new List<TextureSlot>();
        var parameters = view.Follow(shader + ShaderParameters);
        var textures = view.Follow(shader + ShaderTexturesPointer);

        if (!parameters.HasValue || !textures.HasValue)
        {
            return slots;
        }

        var count = view.VirtualBytes(parameters.Value + ParameterTextureCount, 1)[0];

        for (var index = 0; index < count; index++)
        {
            var entry = parameters.Value + ParameterEntries + (index * ParameterEntryBytes);
            var slot = (int)(view.U32(entry + sizeof(uint)) / SlotUnit);
            slots.Add(new TextureSlot(view.U32(entry), TextureHash(view, textures.Value + (slot * sizeof(ulong)))));
        }

        return slots;
    }

    private static ulong? TextureHash(ResourceView view, int pointer)
    {
        var texture = view.Follow(pointer);

        if (!texture.HasValue)
        {
            return null;
        }

        var reference = view.Follow(texture.Value + TextureReference);

        if (!reference.HasValue)
        {
            return null;
        }

        return view.U64(reference.Value + ReferenceHash);
    }

    /// <summary>The objects of a pointer array: the array pointer at +<paramref name="array"/>, its u16 count at +<paramref name="count"/>.</summary>
    private static List<int> Array(ResourceView view, int owner, int array, int count)
    {
        var items = new List<int>();
        var start = view.Follow(owner + array);

        if (!start.HasValue)
        {
            return items;
        }

        for (var index = 0; index < view.U16(owner + count); index++)
        {
            var item = view.Follow(start.Value + (index * sizeof(ulong)));

            if (item.HasValue)
            {
                items.Add(item.Value);
            }
        }

        return items;
    }
}
