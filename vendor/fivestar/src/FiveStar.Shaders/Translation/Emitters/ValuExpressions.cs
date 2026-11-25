// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using static FiveStar.Shaders.Translation.Emitters.Operands;

namespace FiveStar.Shaders.Translation.Emitters;

/// <summary>
/// The HLSL expression of each one-result VALU operation. <c>f(k)</c> reads operand k as float (with modifiers),
/// <c>u(k)</c> as uint, <c>i(k)</c> as int. Operand 0 is the destination, so fmac/mac read it as the addend.
/// </summary>
internal static class ValuExpressions
{
    // Plain multiply-add instead of an exact fused fp32 FMA (the prelude's fma32 is exact but slow on consumer GPUs).
    private const string FusedMultiplyAdd = "mad";

    /// <summary>The float result of a float operation (before output modifiers), or null for other operations.</summary>
    public static string? Float(string op, Func<int, string> f, Func<int, string> u, Func<int, string> i)
    {
        return op switch
        {
            "v_add_f32" => $"{f(Source0)} + {f(Source1)}",
            "v_sub_f32" => $"{f(Source0)} - {f(Source1)}",
            "v_subrev_f32" => $"{f(Source1)} - {f(Source0)}",
            "v_mul_f32" => $"{f(Source0)} * {f(Source1)}",
            "v_mul_legacy_f32" => $"mul_legacy({f(Source0)}, {f(Source1)})",
            "v_fma_f32" => $"{FusedMultiplyAdd}({f(Source0)}, {f(Source1)}, {f(Source2)})",
            "v_mad_f32" => $"mad_unfused({f(Source0)}, {f(Source1)}, {f(Source2)})",
            "v_fma_legacy_f32" or "v_mad_legacy_f32" => $"mad_legacy({f(Source0)}, {f(Source1)}, {f(Source2)})",
            "v_fmac_f32" => $"{FusedMultiplyAdd}({f(Source0)}, {f(Source1)}, asfloat({u(Destination)}))",
            "v_mac_f32" => $"mad_unfused({f(Source0)}, {f(Source1)}, asfloat({u(Destination)}))",
            "v_fmac_legacy_f32" or "v_mac_legacy_f32" => $"mad_legacy({f(Source0)}, {f(Source1)}, asfloat({u(Destination)}))",

            // llvm-objdump order: fmaak = vdst, src0, vsrc1, K; fmamk = vdst, src0, K, vsrc1 (src0 * K + vsrc1).
            "v_fmaak_f32" or "v_fmamk_f32" => $"mad({f(Source0)}, {f(Source1)}, {f(Source2)})",
            "v_madak_f32" or "v_madmk_f32" => $"mad_unfused({f(Source0)}, {f(Source1)}, {f(Source2)})",
            "v_min_f32" => $"min({f(Source0)}, {f(Source1)})",
            "v_max_f32" => $"max({f(Source0)}, {f(Source1)})",
            "v_min3_f32" => $"min(min({f(Source0)}, {f(Source1)}), {f(Source2)})",
            "v_max3_f32" => $"max(max({f(Source0)}, {f(Source1)}), {f(Source2)})",
            "v_med3_f32" => $"med3({f(Source0)}, {f(Source1)}, {f(Source2)})",
            _ => Transcendental(op, f, u, i),
        };
    }

    private static string? Transcendental(string op, Func<int, string> f, Func<int, string> u, Func<int, string> i)
    {
        return op switch
        {
            "v_rcp_f32" or "v_rcp_iflag_f32" => $"rcp({f(Source0)})",
            "v_rsq_f32" => $"rsqrt({f(Source0)})",
            "v_sqrt_f32" => $"sqrt({f(Source0)})",
            "v_exp_f32" => $"exp2({f(Source0)})",
            "v_log_f32" => $"log2({f(Source0)})",

            // v_sin/v_cos take the angle in revolutions.
            "v_sin_f32" => $"sin({f(Source0)} * 6.2831853)",
            "v_cos_f32" => $"cos({f(Source0)} * 6.2831853)",
            "v_fract_f32" => $"fract_f32({f(Source0)})",
            "v_floor_f32" => $"floor({f(Source0)})",
            "v_ceil_f32" => $"ceil({f(Source0)})",
            "v_trunc_f32" => $"trunc({f(Source0)})",
            "v_rndne_f32" => $"round_ne({f(Source0)})",
            "v_ldexp_f32" => $"ldexp_exact({f(Source0)}, {i(Source1)})",
            "v_cvt_f32_u32" => $"(float){u(Source0)}",
            "v_cvt_f32_i32" => $"(float){i(Source0)}",
            "v_cvt_f32_ubyte0" => $"(float)({u(Source0)} & 0xff)",
            "v_cvt_f32_ubyte1" => $"(float)(({u(Source0)} >> 8) & 0xff)",
            "v_cvt_f32_ubyte2" => $"(float)(({u(Source0)} >> 16) & 0xff)",
            "v_cvt_f32_ubyte3" => $"(float)({u(Source0)} >> 24)",
            "v_cvt_f32_f16" => $"f16tof32({u(Source0)})",
            "v_cvt_off_f32_i4" => $"((float)((asint({u(Source0)} << 28)) >> 28) / 16.0)",
            _ => null,
        };
    }

    /// <summary>The uint result of an integer or conversion operation, or null for other operations.</summary>
    public static string? Integer(string op, Func<int, string> f, Func<int, string> u, Func<int, string> i)
    {
        return op switch
        {
            "v_cvt_u32_f32" => $"({f(Source0)} >= 4294967296.0 ? 0xffffffffu : (uint)max({f(Source0)}, 0.0))",
            "v_cvt_i32_f32" => $"asuint((int){f(Source0)})",
            "v_cvt_flr_i32_f32" => $"asuint((int)floor({f(Source0)}))",
            "v_cvt_rpi_i32_f32" => $"asuint((int)floor({f(Source0)} + 0.5))",
            "v_cvt_f16_f32" => $"f32tof16({f(Source0)})",
            "v_cvt_pkrtz_f16_f32" => $"(f32tof16_rtz({f(Source0)}) | (f32tof16_rtz({f(Source1)}) << 16))",
            "v_cvt_pknorm_u16_f32" => $"((uint)round(saturate({f(Source0)}) * 65535.0) | ((uint)round(saturate({f(Source1)}) * 65535.0) << 16))",
            "v_cvt_pk_u16_u32" => $"(min({u(Source0)}, 0xffffu) | (min({u(Source1)}, 0xffffu) << 16))",
            "v_and_b32" => $"{u(Source0)} & {u(Source1)}",
            "v_or_b32" => $"{u(Source0)} | {u(Source1)}",
            "v_xor_b32" => $"{u(Source0)} ^ {u(Source1)}",
            "v_xnor_b32" => $"~({u(Source0)} ^ {u(Source1)})",
            "v_not_b32" => $"~{u(Source0)}",
            "v_or3_b32" => $"{u(Source0)} | {u(Source1)} | {u(Source2)}",
            "v_and_or_b32" => $"({u(Source0)} & {u(Source1)}) | {u(Source2)}",
            "v_lshl_or_b32" => $"({u(Source0)} << ({u(Source1)} & 31)) | {u(Source2)}",
            "v_lshlrev_b32" => $"{u(Source1)} << ({u(Source0)} & 31)",
            "v_lshrrev_b32" => $"{u(Source1)} >> ({u(Source0)} & 31)",
            "v_ashrrev_i32" => $"asuint({i(Source1)} >> ({u(Source0)} & 31))",
            "v_lshl_add_u32" => $"({u(Source0)} << ({u(Source1)} & 31)) + {u(Source2)}",
            "v_add_lshl_u32" => $"({u(Source0)} + {u(Source1)}) << ({u(Source2)} & 31)",
            _ => BitAndArithmetic(op, u, i),
        };
    }

    private static string? BitAndArithmetic(string op, Func<int, string> u, Func<int, string> i)
    {
        return op switch
        {
            "v_bfe_u32" => $"bfe_u({u(Source0)}, {u(Source1)}, {u(Source2)})",
            "v_bfe_i32" => $"bfe_i({u(Source0)}, {u(Source1)}, {u(Source2)})",
            "v_bfi_b32" => $"({u(Source0)} & {u(Source1)}) | (~{u(Source0)} & {u(Source2)})",
            "v_bfm_b32" => $"(((1u << ({u(Source0)} & 31)) - 1) << ({u(Source1)} & 31))",
            "v_bfrev_b32" => $"reversebits({u(Source0)})",
            "v_bcnt_u32_b32" => $"countbits({u(Source0)}) + {u(Source1)}",
            "v_ffbh_u32" => $"firstbithigh_amd({u(Source0)})",
            "v_ffbl_b32" => $"firstbitlow({u(Source0)})",
            "v_alignbit_b32" => $"alignbit({u(Source0)}, {u(Source1)}, {u(Source2)})",
            "v_perm_b32" => $"perm_b32({u(Source0)}, {u(Source1)}, {u(Source2)})",
            "v_add_nc_u32" => $"{u(Source0)} + {u(Source1)}",
            "v_sub_nc_u32" => $"{u(Source0)} - {u(Source1)}",
            "v_subrev_nc_u32" => $"{u(Source1)} - {u(Source0)}",
            "v_mul_lo_u32" => $"{u(Source0)} * {u(Source1)}",
            "v_mul_hi_u32" => $"mul_hi_u({u(Source0)}, {u(Source1)})",
            "v_mul_u32_u24" => $"({u(Source0)} & 0xffffff) * ({u(Source1)} & 0xffffff)",
            "v_mul_i32_i24" => $"asuint(sext24({u(Source0)}) * sext24({u(Source1)}))",
            "v_mad_u32_u24" => $"({u(Source0)} & 0xffffff) * ({u(Source1)} & 0xffffff) + {u(Source2)}",
            "v_mad_i32_i24" => $"asuint(sext24({u(Source0)}) * sext24({u(Source1)}) + {i(Source2)})",
            "v_min_u32" => $"min({u(Source0)}, {u(Source1)})",
            "v_max_u32" => $"max({u(Source0)}, {u(Source1)})",
            "v_min_i32" => $"asuint(min({i(Source0)}, {i(Source1)}))",
            "v_max_i32" => $"asuint(max({i(Source0)}, {i(Source1)}))",
            "v_med3_u32" => $"med3u({u(Source0)}, {u(Source1)}, {u(Source2)})",
            "v_med3_i32" => $"asuint(med3i({i(Source0)}, {i(Source1)}, {i(Source2)}))",
            "v_min3_u32" => $"min(min({u(Source0)}, {u(Source1)}), {u(Source2)})",
            "v_max3_u32" => $"max(max({u(Source0)}, {u(Source1)}), {u(Source2)})",
            "v_readfirstlane_b32" => $"readfirstlane({u(Source0)}, active)",
            _ => null,
        };
    }
}
