// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Tests.Reference;

namespace FiveStar.Shaders.Tests.Cases;

/// <summary>Pixel-shader cases for packed f16 (VOP3P), mixed-precision FMA and SDWA sub-dword selects.</summary>
public static class PixelPackedCases
{
    public static IEnumerable<PixelCase> All()
    {
        return
        [
            PackedHalfMath(),
            FmaMix(),
            Sdwa(),
        ];
    }

    private static uint H(double value) => FloatBits.Half(value);

    // VOP3P: the low lane uses op_sel, the high lane op_sel_hi (default [1,1,1]).
    private static PixelCase PackedHalfMath()
    {
        const string Program = """
            v_mov_b32 v10, s1
            v_mov_b32 v11, s2
            v_pk_fma_f16 v20, s0, v10, v11
            v_pk_fma_f16 v21, s0, v10, v11 op_sel:[1,0,0] op_sel_hi:[0,1,1]
            v_pk_fma_f16 v22, s0, v10, v11 neg_lo:[0,0,1]
            v_pk_add_f16 v23, s0, v10
            v_pk_mul_f16 v24, s0, v10 op_sel_hi:[0,0]
            v_pk_fma_f16 v25, s0, v10, v11 op_sel:[0,0,1] op_sel_hi:[1,1,0]
            """;
        uint[] sources = [FloatBits.Pack(H(1.5), H(2.0)), FloatBits.Pack(H(3.0), H(-1.0)), FloatBits.Pack(H(0.25), H(10.0))];
        uint[] two = [sources[0], sources[1]];

        return PixelCase.Uniform(
            "pk_f16",
            Program,
            [PackedHalf.Operate(PackedOperation.Fma, sources), PackedHalf.Operate(PackedOperation.Fma, sources, [1, 0, 0], [0, 1, 1]),
             PackedHalf.Operate(PackedOperation.Fma, sources, negateLow: [0, 0, 1]), PackedHalf.Operate(PackedOperation.Add, two),
             PackedHalf.Operate(PackedOperation.Multiply, two, [0, 0], [0, 0]), PackedHalf.Operate(PackedOperation.Fma, sources, [0, 0, 1], [1, 1, 0])],
            sources);
    }

    // v_fma_mixlo / mixhi write one half and preserve the other.
    private static PixelCase FmaMix()
    {
        const string Program = """
            v_mov_b32 v10, s1
            v_mov_b32 v12, s2
            v_fma_mix_f32 v20, s0, v10, v12 op_sel_hi:[1,1,0]
            v_fma_mix_f32 v21, s0, v10, v12 op_sel:[1,1,0] op_sel_hi:[1,1,0]
            v_mov_b32 v22, s3
            v_fma_mixlo_f16 v22, s0, v10, v12 op_sel_hi:[1,1,0]
            v_mov_b32 v23, s4
            v_fma_mixhi_f16 v23, s0, v10, v12 op_sel_hi:[1,1,0]
            v_fma_mix_f32 v24, s0, v12, v12
            """;
        uint[] sources = [FloatBits.Pack(H(1.5), H(2.0)), FloatBits.Pack(H(3.0), H(-1.0)), FloatBits.Of(0.5)];
        var low = PackedHalf.FmaMix(sources, [0, 0, 0], [1, 1, 0]);
        var high = PackedHalf.FmaMix(sources, [1, 1, 0], [1, 1, 0]);
        var single = PackedHalf.FmaMix([sources[0], FloatBits.Of(0.5), FloatBits.Of(0.5)], [0, 0, 0], [0, 0, 0]);

        return PixelCase.Uniform(
            "fma_mix",
            Program,
            [FloatBits.Of(low), FloatBits.Of(high), 0xDEAD0000 | FloatBits.Half(low), 0x0000BEEF | (FloatBits.Half(low) << 16), FloatBits.Of(single)],
            [.. sources, 0xDEAD0000, 0x0000BEEF]);
    }

    // Byte / word source selects, sext, dst_sel with PAD / PRESERVE / SEXT.
    private static PixelCase Sdwa()
    {
        const string Program = """
            v_mov_b32 v10, s0
            v_mov_b32 v11, s2
            v_mov_b32 v13, 0
            v_mov_b32_sdwa v20, v10 dst_sel:DWORD dst_unused:UNUSED_PAD src0_sel:BYTE_2
            v_add_nc_u32_sdwa v21, v10, v11 dst_sel:WORD_1 dst_unused:UNUSED_PAD src0_sel:BYTE_1 src1_sel:BYTE_3
            v_mov_b32 v22, s1
            v_add_nc_u32_sdwa v22, v10, v11 dst_sel:BYTE_0 dst_unused:UNUSED_PRESERVE src0_sel:BYTE_0 src1_sel:BYTE_3
            v_cvt_f32_i32_sdwa v23, sext(v10) dst_sel:DWORD dst_unused:UNUSED_PAD src0_sel:BYTE_2
            v_add_nc_u32_sdwa v24, v10, v13 dst_sel:BYTE_0 dst_unused:UNUSED_SEXT src0_sel:BYTE_3 src1_sel:DWORD
            v_add_nc_u32_sdwa v25, sext(v10), v13 dst_sel:DWORD dst_unused:UNUSED_PAD src0_sel:WORD_1 src1_sel:DWORD
            """;

        return PixelCase.Uniform(
            "sdwa",
            Program,
            [0x80, (0x7F + 0x05) << 16, 0x11112200 | (0x01 + 0x05), FloatBits.Of(-128.0), 0xFFFFFF81, 0xFFFF8180],
            0x81807F01, 0x11112222, 0x05000000);
    }
}
