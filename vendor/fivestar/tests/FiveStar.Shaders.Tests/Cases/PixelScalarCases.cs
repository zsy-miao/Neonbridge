// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Numerics;
using FiveStar.Shaders.Tests.Reference;

namespace FiveStar.Shaders.Tests.Cases;

/// <summary>Pixel-shader cases for compares, SALU (with SCC side effects and 64-bit carries), carries, med3 and cube maps.</summary>
public static class PixelScalarCases
{
    private static readonly double[][] CubeVectors =
    [
        [2.0, 0.5, -0.25], [-2.0, 0.5, -0.25], [0.5, 2.0, -0.25], [0.5, -2.0, -0.25], [0.5, -0.25, 2.0], [0.5, -0.25, -2.0], [1.0, 1.0, 1.0],
    ];

    public static IEnumerable<PixelCase> All()
    {
        return
        [
            CompareSelect(),
            CompareClass(),
            Salu(),
            SaluScc(),
            Carries(),
            SaluCarry64(),
            Median(),
            Cube(),
        ];
    }

    private static uint F(double value) => FloatBits.Of(value);

    // lg is ordered not-equal (false on NaN); neq / nlg / nge are unordered (true on NaN).
    private static PixelCase CompareSelect()
    {
        const string Program = """
            v_mov_b32 v10, 20
            v_mov_b32 v11, s1
            v_mov_b32 v12, s2
            v_cmp_lt_f32_e32 vcc_lo, s0, v11
            v_cndmask_b32_e32 v20, 10, v10, vcc_lo
            v_cmp_lg_f32_e32 vcc_lo, s0, v12
            v_cndmask_b32_e32 v21, 10, v10, vcc_lo
            v_cmp_neq_f32_e32 vcc_lo, s0, v12
            v_cndmask_b32_e32 v22, 10, v10, vcc_lo
            v_cmp_nlg_f32_e32 vcc_lo, s0, v12
            v_cndmask_b32_e32 v23, 10, v10, vcc_lo
            v_mov_b32 v13, s3
            v_cmp_gt_i32_e64 s10, -1, v13
            v_cndmask_b32_e64 v24, 0, 1, s10
            v_cmp_lt_u32_e64 s11, s4, v10
            v_cndmask_b32_e64 v25, 0, 1, s11
            v_cmp_u_f32_e64 s12, s0, v12
            v_cndmask_b32_e64 v26, 0, 1, s12
            v_cmp_lg_f32_e32 vcc_lo, s0, v11
            v_cndmask_b32_e32 v27, 10, v10, vcc_lo
            v_cmp_nge_f32_e32 vcc_lo, s0, v12
            v_cndmask_b32_e32 v28, 10, v10, vcc_lo
            """;

        return PixelCase.Uniform(
            "cmp_cndmask", Program, [20, 10, 20, 20, 1, 0, 1, 20, 20], F(1), F(2), FloatBits.QuietNan, unchecked((uint)-5), 0xFFFFFFFF);
    }

    private static PixelCase CompareClass()
    {
        const string Program = """
            v_cmp_class_f32_e64 s10, s0, 0x20
            v_cndmask_b32_e64 v20, 0, 1, s10
            v_cmp_class_f32_e64 s10, s1, 1
            v_cndmask_b32_e64 v21, 0, 1, s10
            v_cmp_class_f32_e64 s10, s1, 2
            v_cndmask_b32_e64 v22, 0, 1, s10
            v_cmp_class_f32_e64 s10, s2, 0x80
            v_cndmask_b32_e64 v23, 0, 1, s10
            v_cmp_class_f32_e64 s10, s3, 4
            v_cndmask_b32_e64 v24, 0, 1, s10
            v_cmp_class_f32_e64 s10, s4, 0x100
            v_cndmask_b32_e64 v25, 0, 1, s10
            v_cmp_class_f32_e64 s10, s5, 2
            v_cndmask_b32_e64 v26, 0, 1, s10
            v_cmp_class_f32_e64 s10, s0, 0x40
            v_cndmask_b32_e64 v27, 0, 1, s10
            """;

        return PixelCase.Uniform(
            "cmp_class",
            Program,
            [IsaFloat.CompareClass(0x80000000, 0x20), IsaFloat.CompareClass(FloatBits.SignalingNan, 1), IsaFloat.CompareClass(FloatBits.SignalingNan, 2),
             IsaFloat.CompareClass(1, 0x80), IsaFloat.CompareClass(FloatBits.NegativeInfinity, 4), IsaFloat.CompareClass(F(1), 0x100),
             IsaFloat.CompareClass(FloatBits.QuietNan, 2), IsaFloat.CompareClass(0x80000000, 0x40)],
            0x80000000, FloatBits.SignalingNan, 0x00000001, FloatBits.NegativeInfinity, F(1), FloatBits.QuietNan);
    }

    // s_bfe operand: offset = S1[4:0], width = S1[22:16]; carries go through SCC.
    private static PixelCase Salu()
    {
        const string Body = """
            s_bfe_u32 s20, s0, s1
            s_bfe_i32 s21, s2, s3
            s_bfe_i32 s22, s4, 0x4000c
            s_bfe_u32 s23, s0, 0x100000
            s_add_u32 s24, s5, 1
            s_addc_u32 s25, s6, 0
            s_sub_u32 s26, 0, 1
            s_subb_u32 s27, 10, 0
            s_cmp_lt_i32 s2, 0
            s_cselect_b32 s28, 7, 8
            s_cmp_lt_u32 s2, 0
            s_cselect_b32 s29, 7, 8
            s_lshl_b32 s30, s0, 36
            s_lshr_b32 s31, s2, 31
            s_ashr_i32 s32, s2, 4
            s_mul_i32 s33, s0, -3
            s_lshl2_add_u32 s34, s6, s0
            s_pack_lh_b32_b16 s35, s0, s2
            s_brev_b32 s36, s0
            s_bcnt1_i32_b32 s37, s0
            s_ff1_i32_b32 s38, s4
            s_not_b32 s39, s0
            """;
        Expected[] expected =
        [
            IsaInteger.ScalarBitFieldUnsigned(0x12345678, 0x80004), IsaInteger.ScalarBitFieldSigned(0x80000000, (16 << 16) | 24),
            IsaInteger.ScalarBitFieldSigned(0xF000, 0x4000C), IsaInteger.ScalarBitFieldUnsigned(0x12345678, 0x100000), 0, 6, 0xFFFFFFFF, 9, 7, 8,
            0x12345678u << 4, 1, unchecked((uint)(IsaInteger.AsInt(0x80000000) >> 4)), unchecked(0x12345678u * (uint)-3), (5u << 2) + 0x12345678,
            0x5678u | 0x80000000, IsaInteger.Reverse(0x12345678), BitOperations.PopCount(0x12345678), 12, ~0x12345678u,
        ];

        return PixelCase.Uniform(
            "salu", Body + "\n" + ScalarToVector(20, 20), expected, 0x12345678, 0x80004, 0x80000000, (16 << 16) | 24, 0xF000, 0xFFFFFFFF, 5);
    }

    // SCC side effects: s_bfe / s_not / s_ashr set SCC = (D != 0); s_min_u32 SCC = S0 < S1; s_max_u32 SCC = S0 > S1;
    // s_add_i32 SCC = signed overflow.
    private static PixelCase SaluScc()
    {
        const string Body = """
            s_cmp_eq_u32 s0, s0
            s_bfe_u32 s20, s0, 0x10010
            s_cselect_b32 s21, 1, 2
            s_cmp_eq_u32 s0, s0
            s_min_u32 s22, s1, s0
            s_cselect_b32 s23, 1, 2
            s_cmp_lg_u32 s0, s0
            s_not_b32 s24, 0
            s_cselect_b32 s25, 1, 2
            s_cmp_eq_u32 s0, s0
            s_ashr_i32 s26, 1, 1
            s_cselect_b32 s27, 1, 2
            s_cmp_lg_u32 s0, s0
            s_add_i32 s28, s2, 1
            s_cselect_b32 s29, 1, 2
            s_cmp_lg_u32 s0, s0
            s_max_u32 s30, s1, s0
            s_cselect_b32 s31, 1, 2
            """;

        return PixelCase.Uniform(
            "salu_scc", Body + "\n" + ScalarToVector(20, 12), [0, 2, 0x1234, 2, 0xFFFFFFFF, 1, 0, 2, 0x80000000, 1, 0x5678, 1], 0x1234, 0x5678, 0x7FFFFFFF);
    }

    private static PixelCase Carries()
    {
        const string Program = """
            v_mov_b32 v10, s0
            v_mov_b32 v11, s1
            v_mov_b32 v12, s2
            v_mov_b32 v13, s4
            v_mov_b32 v14, s3
            v_add_co_u32 v20, vcc_lo, v10, v11
            v_add_co_ci_u32_e32 v21, vcc_lo, 0, v12, vcc_lo
            v_cndmask_b32_e64 v22, 0, 1, vcc_lo
            v_sub_co_u32 v23, vcc_lo, v13, v11
            v_sub_co_ci_u32_e32 v24, vcc_lo, v14, v13, vcc_lo
            v_subrev_co_u32 v25, vcc_lo, v11, v13
            v_cndmask_b32_e64 v26, 0, 1, vcc_lo
            v_mul_hi_u32 v27, v10, v12
            v_mad_u64_u32 v[28:29], s10, v10, v10, v[10:11]
            v_mul_hi_i32 v30, v10, v12
            v_mul_lo_u32 v31, v10, v12
            """;
        var wide = ((UInt128)0xFFFFFFFF * 0xFFFFFFFF) + 0x2FFFFFFFF;

        return PixelCase.Uniform(
            "carries",
            Program,
            [1, 6, 0, 0xFFFFFFFF, 8, 0xFFFFFFFF, 1, (uint)((0xFFFFFFFFUL * 5) >> 32), (uint)wide, (uint)(wide >> 32), unchecked((uint)(-5L >> 32)),
             unchecked(0xFFFFFFFFu * 5)],
            0xFFFFFFFF, 2, 5, 10, 1);
    }

    // 64-bit add / sub through the SCC carry / borrow, 64-bit shifts.
    private static PixelCase SaluCarry64()
    {
        const string Body = """
            s_add_u32 s20, s0, s2
            s_addc_u32 s21, s1, s3
            s_sub_u32 s22, s2, s0
            s_subb_u32 s23, s3, s1
            s_lshl_b64 s[24:25], s[0:1], 4
            s_lshr_b64 s[26:27], s[0:1], 36
            """;
        const ulong Pair = 0x00000001FFFFFFFF;

        return PixelCase.Uniform(
            "salu_carry64",
            Body + "\n" + ScalarToVector(20, 8),
            [1, 1 + 5 + 1, unchecked(2u - 0xFFFFFFFF), 5 - 1 - 1, unchecked((uint)(Pair << 4)), (uint)((Pair << 4) >> 32), (uint)(Pair >> 36), 0],
            0xFFFFFFFF, 0x00000001, 0x00000002, 0x00000005);
    }

    private static PixelCase Median()
    {
        const string Program = """
            v_mov_b32 v10, s2
            v_med3_f32 v20, s0, s1, v10
            v_mov_b32 v11, s5
            v_med3_i32 v21, s3, s4, v11
            v_mov_b32 v12, s8
            v_med3_u32 v22, s6, s7, v12
            v_min3_f32 v23, s0, s1, v10
            v_max3_u32 v24, s6, s7, v12
            v_med3_f32 v25, -s1, s0, v10
            """;

        return PixelCase.Uniform(
            "med3_minmax",
            Program,
            [F(IsaFloat.Median(1, 3, 2)), unchecked((uint)IsaFloat.Median(-5L, 7, 0)), (uint)IsaFloat.Median(5L, 0xFFFFFFFF, 3), F(1), 0xFFFFFFFF,
             F(IsaFloat.Median(-3, 1, 2))],
            F(1), F(3), F(2), unchecked((uint)-5), 7, 0, 5, 0xFFFFFFFF, 3);
    }

    // (id, sc, tc, ma) for each face, and the x = y = z tie (z wins).
    private static PixelCase Cube()
    {
        var program = new List<string>();
        var expected = new List<Expected>();
        var user = new List<uint>();

        for (var index = 0; index < CubeVectors.Length; index++)
        {
            var (x, y, z) = (CubeVectors[index][0], CubeVectors[index][1], CubeVectors[index][2]);
            var first = 20 + (4 * index);
            program.Add($"v_mov_b32 v10, s{3 * index}\nv_mov_b32 v11, s{(3 * index) + 1}\nv_mov_b32 v12, s{(3 * index) + 2}");
            program.Add($"v_cubeid_f32 v{first}, v10, v11, v12\nv_cubesc_f32 v{first + 1}, v10, v11, v12");
            program.Add($"v_cubetc_f32 v{first + 2}, v10, v11, v12\nv_cubema_f32 v{first + 3}, v10, v11, v12");

            expected.AddRange([F(CubeMap.Face(x, y, z)), F(CubeMap.S(x, y, z)), F(CubeMap.T(x, y, z)), F(CubeMap.MajorAxis(x, y, z))]);
            user.AddRange([F(x), F(y), F(z)]);
        }

        return PixelCase.Uniform("cube", string.Join("\n", program), [.. expected], [.. user]);
    }

    /// <summary>v_mov_b32 v{first + k}, s{first + k} for the results the SALU cases leave in SGPRs.</summary>
    private static string ScalarToVector(int first, int count)
    {
        return string.Join("\n", Enumerable.Range(first, count).Select(register => $"v_mov_b32 v{register}, s{register}"));
    }
}
