// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Tests.Warp;
using Vortice.DXGI;

namespace FiveStar.Shaders.Tests.Cases;

/// <summary>
/// Typed image views (TranslationOptions.TypedImageViews): the T# in s[8:15] carries the view's class in dword 3 (s11), as
/// the loader writes it; each thread reads or writes texel (t &amp; 3, (t &gt;&gt; 2) &amp; 1), index t &amp; 7, of a 4x2 texture.
/// </summary>
public static class TypedViewCases
{
    private const int TextureHeap = 5;
    private const int Width = 4;
    private const int Height = 2;
    private const int Texels = Width * Height;
    private const uint Uint = 1;
    private const uint Sint = 2;
    private const uint Float = 0;

    private const string Address = """
        v_and_b32_e32 v1, 3, v41
        v_bfe_u32 v2, v41, 2, 1
        """;

    public static IEnumerable<ComputeCase> All()
    {
        return [UintLoad(), SintLoad(), FloatLoad(), HalfLoad(), UintStore()];
    }

    // NaN and sign patterns a float view could turn into other bits: an integer view returns them as they are.
    private static ComputeCase UintLoad()
    {
        uint[] texels = [.. Enumerable.Range(0, Texels).Select(index => 0x7FC00001u + (uint)index * 0x10001u)];

        return Load("cs_typed_uint_load", Format.R32_UInt, Uint, texels);
    }

    private static ComputeCase SintLoad()
    {
        uint[] texels = [.. Enumerable.Range(0, Texels).Select(index => unchecked((uint)(-1 - (index * 1000))))];

        return Load("cs_typed_sint_load", Format.R32_SInt, Sint, texels);
    }

    // Class 0 is the float view as before.
    private static ComputeCase FloatLoad()
    {
        uint[] texels = [.. Enumerable.Range(0, Texels).Select(index => BitConverter.SingleToUInt32Bits(0.25f * index))];

        return Load("cs_typed_float_load", Format.R32_Float, Float, texels);
    }

    // d16 of an integer view: each value's low 16 bits, two per register.
    private static ComputeCase HalfLoad()
    {
        var texels = new uint[2 * Texels];

        for (var index = 0; index < Texels; index++)
        {
            (texels[2 * index], texels[(2 * index) + 1]) = (0x12340000u + (uint)index, 0xABCD8000u + (uint)index);
        }

        var program = Address + "\n" + """
            image_load v20, v[1:2], s[8:15] dmask:0x3 dim:SQ_RSRC_IMG_2D unorm d16
            s_waitcnt vmcnt(0)
            """;

        return new ComputeCase("cs_typed_d16_load", program, 1, thread => [Expected.Exact((0x8000u + (uint)(thread.Id & 7)) << 16 | (uint)(thread.Id & 7))])
        {
            User = [TextureHeap, 0, 0, Uint],
            Textures = [new TextureResource(TextureHeap, Format.R32G32_UInt, Width, Height, texels)],
        };
    }

    // A store through an integer view writes the register's bits; through a float4 declaration it would convert them.
    private static ComputeCase UintStore()
    {
        var program = Address + "\n" + """
            v_and_b32_e32 v3, 7, v41
            v_or_b32_e32 v3, 0xffff0000, v3
            image_store v3, v[1:2], s[8:15] dmask:0x1 dim:SQ_RSRC_IMG_2D unorm
            v_mov_b32 v20, v3
            s_waitcnt vmcnt(0)
            """;
        uint[] expected = [.. Enumerable.Range(0, Texels).Select(index => 0xFFFF0000u | (uint)index)];

        return new ComputeCase("cs_typed_uint_store", program, 1, thread => [Expected.Exact(0xFFFF0000u | (uint)(thread.Id & 7))])
        {
            User = [TextureHeap, 0, 0, Uint],
            Textures = [new TextureResource(TextureHeap, Format.R32_UInt, Width, Height, new uint[Texels]) { Writable = true }],
            Check = written => Enumerable.Range(0, Texels).Where(index => written[TextureHeap][index] != expected[index]).Select(index => $"texel {index} expected {Expected.Hex(expected[index])} actual {Expected.Hex(written[TextureHeap][index])}"),
        };
    }

    private static ComputeCase Load(string name, Format format, uint viewClass, uint[] texels)
    {
        var program = Address + "\n" + """
            image_load v20, v[1:2], s[8:15] dmask:0x1 dim:SQ_RSRC_IMG_2D unorm
            s_waitcnt vmcnt(0)
            """;

        return new ComputeCase(name, program, 1, thread => [Expected.Exact(texels[thread.Id & 7])])
        {
            User = [TextureHeap, 0, 0, viewClass],
            Textures = [new TextureResource(TextureHeap, format, Width, Height, texels)],
        };
    }
}
