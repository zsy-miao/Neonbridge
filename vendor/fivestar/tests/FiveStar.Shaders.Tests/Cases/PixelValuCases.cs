// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Tests.Reference;

namespace FiveStar.Shaders.Tests.Cases;

/// <summary>Pixel-shader cases for float, f16, integer and conversion VALU instructions.</summary>
public static class PixelValuCases
{
    public static IEnumerable<PixelCase> All()
    {
        return
        [
            MulLegacy(),
            FmaMac(),
            Transcendental(),
            SinLargeArgument(),
            RoundLdexp(),
            Modifiers(),
            HalfArithmetic(),
            HalfInlineConstants(),
            Multiply24(),
            BitField(),
            AlignPermute(),
            ConvertInt(),
            ConvertHalfByte(),
            ConvertPack(),
        ];
    }

    private static uint F(double value) => FloatBits.Of(value);

    private static double V(uint bits) => FloatBits.Value(bits);

    // A v_fma_f32 result, through mad(): fused as on AMD, or unfused where the device does not fuse (WARP).
    private static Expected Fused(double a, double b, double c) => Expected.Either(IsaFloat.Fma(a, b, c), IsaFloat.Mad(a, b, c));

    // 0 * inf = 0 and 0 * NaN = 0 under the DX9 legacy rules; plain v_mul_f32 gives NaN.
    private static PixelCase MulLegacy()
    {
        const string Program = """
            v_mov_b32 v10, s1
            v_mul_legacy_f32_e32 v20, s0, v10
            v_mov_b32 v11, s2
            v_mul_legacy_f32_e32 v21, s0, v11
            v_mov_b32 v12, s3
            v_mul_legacy_f32_e32 v22, s1, v12
            v_mul_f32_e32 v23, s0, v10
            v_mov_b32 v24, 1.0
            v_mac_legacy_f32_e32 v24, s0, v10
            v_mul_legacy_f32_e32 v25, s3, v12
            """;
        var inf = double.PositiveInfinity;

        return PixelCase.Uniform(
            "mul_legacy",
            Program,
            [IsaFloat.MulLegacy(0, inf), IsaFloat.MulLegacy(0, double.NaN), IsaFloat.MulLegacy(inf, 3), Expected.Nan,
             F(1.0 + V(IsaFloat.MulLegacy(0, inf))), IsaFloat.MulLegacy(3, 3)],
            F(0), FloatBits.Infinity, FloatBits.QuietNan, F(3));
    }

    // (1 + 2^-23)(1 - 2^-23) - 1 is -2^-46 fused and 0 unfused. v_fma_f32 / v_fmac_f32 are fused on AMD; the translator
    // emits mad() (docs/shader-translation.md, FMA), which NVIDIA fuses and WARP does not, so v20 / v21 may be either.
    // v22 / v23 (v_mad / v_mac) are unfused on AMD and must stay so on every device.
    private static PixelCase FmaMac()
    {
        const string Program = """
            v_mov_b32 v10, s1
            v_fma_f32 v20, s0, v10, -1.0
            v_mov_b32 v21, -1.0
            v_fmac_f32_e32 v21, s0, v10
            v_mad_f32 v22, s0, v10, -1.0
            v_mov_b32 v23, -1.0
            v_mac_f32_e32 v23, s0, v10
            v_mov_b32 v11, s3
            v_fma_f32 v24, s2, v11, 1.0
            v_fmaak_f32 v25, s2, v11, 0x3e800000
            v_fmamk_f32 v26, s2, 0x3e800000, v11
            """;
        var (a, b) = (V(0x3F800001), V(0x3F7FFFFE));

        return PixelCase.Uniform(
            "fma_mac",
            Program,
            [Fused(a, b, -1), Fused(a, b, -1), IsaFloat.Mad(a, b, -1), IsaFloat.Mad(a, b, -1), IsaFloat.Fma(2, 3, 1),
             IsaFloat.Fma(2, 3, 0.25), IsaFloat.Fma(2, 0.25, 3)],
            0x3F800001, 0x3F7FFFFE, F(2), F(3));
    }

    // v_sin / v_cos take revolutions (sin(2 pi x)); 1-ulp / 2e-6 tolerance.
    private static PixelCase Transcendental()
    {
        const string Program = """
            v_rcp_f32_e32 v20, s0
            v_rsq_f32_e32 v21, s1
            v_sqrt_f32_e32 v22, s2
            v_exp_f32_e32 v23, s0
            v_log_f32_e32 v24, s5
            v_sin_f32_e32 v25, s3
            v_cos_f32_e32 v26, s4
            v_sin_f32_e32 v27, s6
            v_cos_f32_e32 v28, s3
            v_rcp_f32_e32 v29, 0
            v_rsq_f32_e32 v30, -1.0
            v_log_f32_e32 v31, 0
            """;

        return PixelCase.Uniform(
            "transcendental",
            Program,
            [Expected.Ulp(F(1.0 / 3), 1), Expected.Ulp(F(0.5), 1), Expected.Ulp(F(Math.Sqrt(2)), 1), Expected.Ulp(F(8), 1),
             Expected.Ulp(F(3), 1), Expected.Near(1, 2e-6), Expected.Near(-1, 2e-6), Expected.Near(Math.Sin(Math.PI / 4), 2e-6),
             Expected.Near(0, 2e-6), FloatBits.Infinity, Expected.Nan, FloatBits.NegativeInfinity],
            F(3), F(4), F(2), F(0.25), F(0.5), F(8), F(0.125));
    }

    // The hardware reduces the argument in revolutions (exactly); |x| up to 256 is in the valid range.
    private static PixelCase SinLargeArgument()
    {
        const string Program = """
            v_sin_f32_e32 v20, s0
            v_cos_f32_e32 v21, s0
            v_sin_f32_e32 v22, s1
            v_sin_f32_e32 v23, s2
            v_cos_f32_e32 v24, s3
            """;

        return PixelCase.Uniform(
            "sin_large_arg",
            Program,
            [Expected.Near(Math.Sin(2 * Math.PI * 0.125), 1e-5), Expected.Near(Math.Cos(2 * Math.PI * 0.125), 1e-5),
             Expected.Near(Math.Sin(2 * Math.PI * -0.375), 1e-5), Expected.Near(Math.Sin(2 * Math.PI * 0.875), 1e-5),
             Expected.Near(Math.Cos(2 * Math.PI * 0.0625), 1e-5)],
            F(100.125), F(-200.375), F(255.875), F(250.0625));
    }

    // rndne rounds half to even (-0.5 gives -0.0); v_fract clamps below 1.0; ldexp is exact.
    private static PixelCase RoundLdexp()
    {
        const string Program = """
            v_fract_f32_e32 v20, s0
            v_rndne_f32_e32 v21, s1
            v_rndne_f32_e32 v22, s2
            v_rndne_f32_e32 v23, s3
            v_floor_f32_e32 v24, s0
            v_ceil_f32_e32 v25, s0
            v_trunc_f32_e32 v26, s0
            v_ldexp_f32 v27, s5, 3
            v_ldexp_f32 v28, s4, 0xc8
            v_ldexp_f32 v29, s5, -2
            v_rndne_f32_e32 v30, s6
            v_fract_f32_e32 v31, s7
            """;

        return PixelCase.Uniform(
            "round_ldexp",
            Program,
            [IsaFloat.Fract(-1.25), IsaFloat.RoundNearestEven(2.5), IsaFloat.RoundNearestEven(3.5), IsaFloat.RoundNearestEven(-2.5), F(-2),
             F(-1), F(-1), IsaFloat.Ldexp(1.5, 3), IsaFloat.Ldexp(Math.Pow(2, -100), 200), IsaFloat.Ldexp(1.5, -2),
             IsaFloat.RoundNearestEven(-0.5), IsaFloat.Fract(V(0xAEDBE6FF))],
            F(-1.25), F(2.5), F(3.5), F(-2.5), F(Math.Pow(2, -100)), F(1.5), F(-0.5), 0xAEDBE6FF);
    }

    // omod / clamp / abs / neg source modifiers; the cndmask selector is an integer SGPR.
    private static PixelCase Modifiers()
    {
        const string Program = """
            v_mul_f32_e64 v20, s0, s1 mul:4
            v_mul_f32_e64 v21, s0, s1 clamp mul:4
            v_add_f32_e64 v22, -|s2|, s0
            v_add_f32_e64 v23, |s2|, s0 div:2
            v_max_f32_e64 v24, -s0, s2 clamp
            v_mov_b32 v10, s2
            v_cndmask_b32_e64 v25, s0, -v10, s4
            v_cndmask_b32_e64 v26, s0, -v10, s5
            """;

        return PixelCase.Uniform(
            "modifiers", Program, [F(3), F(1), F(1), F(2.5), F(0), F(3), F(2)], F(3), F(0.25), F(-2), 0, 0, 0xFFFFFFFF);
    }

    // f16 results are rounded to nearest even (1.0 + 0.75 ulp gives 1.0 + 1 ulp); the high half is pre-zeroed.
    private static PixelCase HalfArithmetic()
    {
        const string Program = """
            v_mov_b32 v20, 0
            v_mov_b32 v21, 0
            v_mov_b32 v22, 0
            v_mov_b32 v23, 0
            v_mov_b32 v10, s1
            v_add_f16_e32 v20, s0, v10
            v_mul_f16_e32 v21, s2, v10
            v_fma_f16 v22, s0, v10, s3
            v_max_f16_e32 v23, s3, v10
            """;
        var h = FloatBits.ExactHalf;

        return PixelCase.Uniform(
            "f16_arith",
            Program,
            [FloatBits.Half(h(0x3C00) + h(0x1200)), FloatBits.Half(h(0x3555) * h(0x1200)), FloatBits.Half((h(0x3C00) * h(0x1200)) + h(0xBC00)), 0x1200],
            0x3C00, 0x1200, 0x3555, 0xBC00);
    }

    // An inline float constant of an f16 operation is the f16 value (1.0 is 0x3C00), not the low half of the f32 one.
    private static PixelCase HalfInlineConstants()
    {
        const string Program = """
            v_mov_b32 v20, 0
            v_mov_b32 v21, 0
            v_mov_b32 v22, 0
            v_mov_b32 v10, s0
            v_add_f16_e32 v20, 1.0, v10
            v_mul_f16_e32 v21, -2.0, v10
            v_fma_f16 v22, v10, 0.5, 4.0
            """;
        var h = FloatBits.ExactHalf;
        var x = h(0x3555);

        return PixelCase.Uniform("f16_inline_constants", Program, [FloatBits.Half(h(0x3C00) + x), FloatBits.Half(h(0xC000) * x), FloatBits.Half((x * h(0x3800)) + h(0x4400))], 0x3555);
    }

    // 24-bit operands: the high byte is ignored, i24 is sign-extended.
    private static PixelCase Multiply24()
    {
        const string Program = """
            v_mov_b32 v10, s1
            v_mul_i32_i24_e32 v20, s0, v10
            v_mul_u32_u24_e32 v21, s0, v10
            v_mad_u32_u24 v22, s0, v10, s2
            v_mad_i32_i24 v23, s0, v10, s2
            v_mul_u32_u24_e32 v24, s1, v10
            """;
        var a = IsaInteger.SignExtend24(0xFFFFFE);
        var b = IsaInteger.SignExtend24(0x12000003);

        return PixelCase.Uniform(
            "mul24",
            Program,
            [unchecked((uint)(a * b)), 0xFFFFFEu * 3, (0xFFFFFEu * 3) + 100, unchecked((uint)((a * 3) + 100)), 9],
            0x00FFFFFE, 0x12000003, 100);
    }

    // v_bfe_i32 with offset + width > 32: the field runs past bit 31, the result is S0.i >> offset.
    private static PixelCase BitField()
    {
        const string Program = """
            v_bfe_u32 v20, s0, 8, 8
            v_bfe_i32 v21, s0, 4, 8
            v_bfe_i32 v22, s1, 28, 8
            v_bfe_u32 v23, s1, 28, 8
            v_bfe_i32 v24, s5, 28, 8
            v_bfe_i32 v25, s0, 0, 0
            v_bfe_i32 v26, s0, 12, 20
            v_mov_b32 v10, s4
            v_bfi_b32 v27, s2, s3, v10
            v_bfm_b32 v28, 4, 8
            v_bfrev_b32_e32 v29, s1
            v_ffbh_u32_e32 v30, s5
            v_ffbl_b32_e32 v31, 0
            v_bcnt_u32_b32 v32, s0, 1
            v_bfe_i32 v33, s6, 20, 16
            """;

        return PixelCase.Uniform(
            "bitfield",
            Program,
            [IsaInteger.BitFieldUnsigned(0x12345678, 8, 8), IsaInteger.BitFieldSigned(0x12345678, 4, 8), IsaInteger.BitFieldSigned(0x80000000, 28, 8),
             IsaInteger.BitFieldUnsigned(0x80000000, 28, 8), IsaInteger.BitFieldSigned(0x40000000, 28, 8), 0, IsaInteger.BitFieldSigned(0x12345678, 12, 20),
             (0xFF00FF00 & 0xAAAAAAAA) | (~0xFF00FF00 & 0x55555555), 0xF00, IsaInteger.Reverse(0x80000000), IsaInteger.FirstBitHigh(0x40000000),
             IsaInteger.FirstBitLow(0), (uint)System.Numerics.BitOperations.PopCount(0x12345678) + 1, IsaInteger.BitFieldSigned(0x9ABCDEF0, 20, 16)],
            0x12345678, 0x80000000, 0xFF00FF00, 0xAAAAAAAA, 0x55555555, 0x40000000, 0x9ABCDEF0);
    }

    private static PixelCase AlignPermute()
    {
        const string Program = """
            v_mov_b32 v10, s1
            v_alignbit_b32 v20, s0, v10, 8
            v_alignbit_b32 v21, s0, v10, 0
            v_alignbit_b32 v22, s0, v10, 36
            v_perm_b32 v23, s0, v10, s2
            v_perm_b32 v24, s0, v10, s3
            v_mov_b32 v11, s6
            v_perm_b32 v25, s5, v11, s4
            """;

        return PixelCase.Uniform(
            "align_perm",
            Program,
            [IsaInteger.AlignBit(0x11223344, 0x55667788, 8), IsaInteger.AlignBit(0x11223344, 0x55667788, 0), IsaInteger.AlignBit(0x11223344, 0x55667788, 36),
             IsaInteger.Permute(0x11223344, 0x55667788, 0x0C0D0800), IsaInteger.Permute(0x11223344, 0x55667788, 0x07050301),
             IsaInteger.Permute(0x81223344, 0x5566F788, 0x0B0A0908)],
            0x11223344, 0x55667788, 0x0C0D0800, 0x07050301, 0x0B0A0908, 0x81223344, 0x5566F788);
    }

    // float to int: NaN gives 0, out of range saturates (u32: negatives to 0, >= 2^32 to 0xffffffff).
    private static PixelCase ConvertInt()
    {
        const string Program = """
            v_cvt_u32_f32_e32 v20, s0
            v_cvt_u32_f32_e32 v21, s1
            v_cvt_u32_f32_e32 v22, s2
            v_cvt_u32_f32_e32 v23, s3
            v_cvt_i32_f32_e32 v24, s4
            v_cvt_i32_f32_e32 v25, s5
            v_cvt_i32_f32_e32 v26, s1
            v_cvt_i32_f32_e32 v27, s6
            v_cvt_flr_i32_f32_e32 v28, s6
            v_cvt_rpi_i32_f32_e32 v29, s6
            v_cvt_f32_u32_e32 v30, s7
            v_cvt_f32_i32_e32 v31, s8
            v_cvt_u32_f32_e32 v32, s10
            v_cvt_i32_f32_e32 v33, s9
            v_cvt_u32_f32_e32 v34, s9
            v_cvt_i32_f32_e32 v35, s10
            """;
        var (inf, nan) = (double.PositiveInfinity, double.NaN);

        return PixelCase.Uniform(
            "cvt_int",
            Program,
            [IsaFloat.ConvertToUInt(-1), IsaFloat.ConvertToUInt(nan), IsaFloat.ConvertToUInt(5e9), IsaFloat.ConvertToUInt(3.75), IsaFloat.ConvertToInt(3e9),
             IsaFloat.ConvertToInt(-3e9), IsaFloat.ConvertToInt(nan), IsaFloat.ConvertToInt(-0.5), IsaFloat.ConvertFloorToInt(-0.5),
             IsaFloat.ConvertRoundToInt(-0.5), F(4294967295.0), F(-2147483648.0), IsaFloat.ConvertToUInt(inf), IsaFloat.ConvertToInt(-inf),
             IsaFloat.ConvertToUInt(-inf), IsaFloat.ConvertToInt(inf)],
            F(-1), FloatBits.QuietNan, F(5e9), F(3.75), F(3e9), F(-3e9), F(-0.5), 0xFFFFFFFF, 0x80000000, FloatBits.NegativeInfinity, FloatBits.Infinity);
    }

    // f32 to f16 is round to nearest even (65520 gives inf); v_cvt_pkrtz rounds toward zero (65520 gives 65504).
    private static PixelCase ConvertHalfByte()
    {
        const string Program = """
            v_cvt_f32_ubyte0_e32 v20, s0
            v_cvt_f32_ubyte1_e32 v21, s0
            v_cvt_f32_ubyte2_e32 v22, s0
            v_cvt_f32_ubyte3_e32 v23, s0
            v_cvt_f16_f32_e32 v24, s1
            v_cvt_f16_f32_e32 v25, s2
            v_cvt_pkrtz_f16_f32_e64 v26, s1, s2
            v_cvt_f32_f16_e32 v27, s3
            v_cvt_f32_f16_e32 v28, s4
            v_cvt_f16_f32_e32 v29, s5
            v_cvt_f16_f32_e32 v30, s6
            v_cvt_pkrtz_f16_f32_e64 v31, s6, s7
            """;
        var (third, twoThirds) = (V(F(1.0 / 3)), V(F(-2.0 / 3)));

        return PixelCase.Uniform(
            "cvt_f16_ubyte",
            Program,
            [F(2), F(1), F(255), F(128), FloatBits.Half(V(0x3F801800)), FloatBits.Half(65520.0),
             FloatBits.Pack(FloatBits.Half(V(0x3F801800), towardZero: true), FloatBits.Half(65520.0, towardZero: true)), F(FloatBits.HalfValue(0x0001)),
             F(FloatBits.HalfValue(0xC000)), FloatBits.Half(Math.Pow(2, -20)), FloatBits.Half(third),
             FloatBits.Pack(FloatBits.Half(third, towardZero: true), FloatBits.Half(twoThirds, towardZero: true))],
            0x80FF0102, 0x3F801800, F(65520.0), 0x0001, 0xC000, F(Math.Pow(2, -20)), F(1.0 / 3), F(-2.0 / 3));
    }

    // v_cvt_pk_u8_f32: round to nearest even and clamp to [0, 255] into the selected byte; pknorm: RNE of saturate(x) * 65535.
    private static PixelCase ConvertPack()
    {
        const string Program = """
            v_mov_b32 v10, s1
            v_cvt_pk_u8_f32 v20, s0, 1, v10
            v_cvt_pk_u8_f32 v21, s2, 3, v10
            v_cvt_pknorm_u16_f32 v22, s3, s4
            v_cvt_pk_u16_u32 v23, s5, 7
            v_cvt_off_f32_i4_e32 v24, 8
            v_cvt_pk_u8_f32 v25, s6, 0, v10
            """;
        var half = (uint)Math.Min(Math.Round(0.5 * 65535, MidpointRounding.ToEven), 65535);

        return PixelCase.Uniform(
            "cvt_pack",
            Program,
            [0x11220244, 0xFF223344, half | (0xFFFFu << 16), 0x0007FFFF, F(-0.5), 0x11223300],
            F(2.5), 0x11223344, F(300), F(0.5), F(1.5), 0x12345, F(-3));
    }
}
