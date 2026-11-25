// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;
using FiveStar.Shaders.Tests.Warp;
using Vortice.DXGI;

namespace FiveStar.Shaders.Tests.Cases;

/// <summary>Compute cases for memory paths the corpus uses: 64-bit image atomics, LDS d16 halves.</summary>
public static class ComputeImageCases
{
    private const int TextureHeap = 5;
    private const int Width = 4;
    private const int Height = 2;

    public static IEnumerable<ComputeCase> All()
    {
        return [WideAtomic(), LdsHalves()];
    }

    // image_atomic_umax with dmask 0x3 is a 64-bit max on an R32G32_UINT texel, its data low dword first (974 of them in
    // the corpus): 64 threads max (t, t >> 2) into texel t & 3 of row 0, whose (5, 1) only the high dwords beat; with glc
    // every thread maxes (t, 0) into (0, 1), which holds (3, 9), and gets (3, 9) back. The DXIL backend lowers it too (an
    // i64 atomicBinOp on a uint64_t texture, 679 alpha-prepass programs of the corpus).
    private static ComputeCase WideAtomic()
    {
        const string Program = """
            v_and_b32_e32 v1, 3, v41
            v_mov_b32 v2, 0
            v_mov_b32 v20, v41
            v_lshrrev_b32_e32 v21, 2, v41
            image_atomic_umax v[20:21], v[1:2], s[8:15] dmask:0x3 dim:SQ_RSRC_IMG_2D unorm
            v_mov_b32 v22, v41
            v_mov_b32 v23, 0
            v_mov_b32 v4, 0
            v_mov_b32 v5, 1
            image_atomic_umax v[22:23], v[4:5], s[8:15] dmask:0x3 dim:SQ_RSRC_IMG_2D unorm glc
            s_waitcnt vmcnt(0)
            """;
        var initial = new uint[2 * Width * Height];
        var final = new uint[initial.Length];

        for (var x = 0; x < Width; x++)
        {
            (initial[2 * x], initial[(2 * x) + 1]) = (5, 1);
            (final[2 * x], final[(2 * x) + 1]) = ((uint)(60 + x), 15);
        }

        (initial[2 * Width], initial[(2 * Width) + 1]) = (3, 9);
        (final[2 * Width], final[(2 * Width) + 1]) = (3, 9);

        return new ComputeCase("cs_image_atomic_64", Program, 4, thread => [Expected.Exact((uint)thread.Id), Expected.Exact((uint)thread.Id >> 2), Expected.Exact(3), Expected.Exact(9)])
        {
            User = [TextureHeap, 0, 0, 0],
            Textures = [new TextureResource(TextureHeap, Format.R32G32_UInt, Width, Height, initial) { Writable = true }],
            Check = written => Differences($"texture{TextureHeap}", final, written[TextureHeap]),
        };
    }

    // ds_write_b16_d16_hi stores a VGPR's high half, ds_read_u16_d16_hi loads into the high half and keeps the low one,
    // ds_read_u16 zero-extends: each thread's dword 0x11112222 gets 0x3000 + t in its high half.
    private static ComputeCase LdsHalves()
    {
        const string Program = """
            v_lshlrev_b32_e32 v1, 2, v41
            v_mov_b32 v3, 0x11112222
            ds_write_b32 v1, v3
            v_add_nc_u32_e32 v4, 0x3000, v41
            v_lshlrev_b32_e32 v4, 16, v4
            v_or_b32_e32 v4, 0x5555, v4
            ds_write_b16_d16_hi v1, v4 offset:2
            s_waitcnt lgkmcnt(0)
            ds_read_b32 v20, v1
            v_mov_b32 v21, 0x7777bbbb
            ds_read_u16_d16_hi v21, v1 offset:2
            ds_read_u16 v22, v1 offset:2
            s_waitcnt lgkmcnt(0)
            """;

        return new ComputeCase("cs_lds_d16_hi", Program, 3, thread =>
        {
            var high = 0x3000 + (uint)thread.Id;

            return [Expected.Exact((high << 16) | 0x2222), Expected.Exact((high << 16) | 0xBBBB), Expected.Exact(high)];
        })
        {
            LdsBytes = 4 * 64,
        };
    }

    private static IEnumerable<string> Differences(string name, uint[] expected, uint[] actual)
    {
        for (var index = 0; index < expected.Length; index++)
        {
            if (expected[index] != actual[index])
            {
                yield return string.Create(CultureInfo.InvariantCulture, $"{name}[{index}] expected {Expected.Hex(expected[index])} actual {Expected.Hex(actual[index])}");
            }
        }
    }
}
