// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Tests.Cases;

/// <summary>Pixel-shader cases for branches, EXEC-masked if / else, loops and discard (raw programs with their own s_endpgm).</summary>
public static class PixelControlFlowCases
{
    public static IEnumerable<PixelCase> All()
    {
        return
        [
            If(),
            IfElse(),
            LongJumps(),
            ScalarLoop(),
            DivergentLoop(),
            NestedExecSave(),
            ExecSaveAfterIntegerWrite(),
            VccBranches(),
            DiscardChecker(),
            DiscardQuad(),
        ];
    }

    // s_and_saveexec + s_cbranch_execz + restore: x < 3 splits the x = 2 / 3 quads, the x >= 4 quads skip entirely.
    private static PixelCase If()
    {
        const string Program = """
            v_cvt_u32_f32_e32 v10, v2
            v_mov_b32 v20, 0
            v_mov_b32 v21, 0
            v_cmp_gt_u32_e32 vcc_lo, 3, v10
            s_and_saveexec_b32 s10, vcc_lo
            s_cbranch_execz .LBB_end
            v_mov_b32 v20, 1
            v_add_nc_u32_e32 v21, 100, v10
            .LBB_end:
            s_or_b32 exec_lo, exec_lo, s10
            v_add_nc_u32_e32 v22, 5, v20
            {EXPORT}
            s_endpgm
            """;

        return new PixelCase("cf_if", Program, 3, IfResult) { Raw = true };
    }

    // LLVM's if / else lowering: s_xor (else mask), s_or_saveexec + s_xor exec, restore.
    private static PixelCase IfElse()
    {
        const string Program = """
            v_cvt_u32_f32_e32 v10, v2
            v_cvt_u32_f32_e32 v11, v3
            v_mov_b32 v20, 0
            v_cmp_gt_u32_e32 vcc_lo, v10, v11
            s_and_saveexec_b32 s10, vcc_lo
            s_xor_b32 s10, exec_lo, s10
            s_cbranch_execz .LBB_else
            v_add_nc_u32_e32 v20, 0x3e8, v10
            .LBB_else:
            s_or_saveexec_b32 s11, s10
            s_xor_b32 exec_lo, exec_lo, s11
            s_cbranch_execz .LBB_end
            v_add_nc_u32_e32 v20, 0x7d0, v11
            .LBB_end:
            s_or_b32 exec_lo, exec_lo, s11
            v_mov_b32 v21, 1
            {EXPORT}
            s_endpgm
            """;

        return new PixelCase("cf_if_else", Program, 2, IfElseResult) { Raw = true };
    }

    // A relaxed branch into far code that returns through long jumps: the forward one as the hardware computes it, the
    // backward one with the console compiler's offset measured from the end of its s_sub_u32 (8 bytes early).
    private static PixelCase LongJumps()
    {
        const string Program = """
            v_cvt_u32_f32_e32 v10, v2
            v_mov_b32 v20, 0
            s_mov_b32 s15, exec_lo
            v_cmpx_gt_u32_e32 3, v10
            s_cbranch_execz .LBB_else
            s_branch .LBB_then
            s_nop 0
            .LBB_else:
            s_andn2_b32 exec_lo, s15, exec_lo
            s_cbranch_execz .LBB_end
            v_mov_b32 v20, 2
            .LBB_end:
            s_mov_b32 exec_lo, s15
            v_mov_b32 v21, 1
            {EXPORT}
            s_endpgm
            .LBB_then:
            v_add_nc_u32_e32 v20, 10, v10
            s_getpc_b64 s[8:9]
            .Lpc_out:
            s_add_u32 s8, s8, .Lreturn-.Lpc_out
            s_addc_u32 s9, s9, 0
            s_setpc_b64 s[8:9]
            .Lreturn:
            s_getpc_b64 s[8:9]
            .Lpc_back:
            s_sub_u32 s8, s8, .Lpc_back-.LBB_else+8
            s_subb_u32 s9, s9, 0
            s_setpc_b64 s[8:9]
            """;

        return new PixelCase("cf_long_jumps", Program, 2, LongJumpResult) { Raw = true };
    }

    private static PixelCase ScalarLoop()
    {
        const string Program = """
            s_mov_b32 s10, 0
            s_mov_b32 s11, 0
            .LBB_loop:
            s_add_u32 s11, s11, s0
            s_add_u32 s10, s10, 1
            s_cmp_lt_u32 s10, s1
            s_cbranch_scc1 .LBB_loop
            v_mov_b32 v20, s11
            v_mov_b32 v21, s10
            {EXPORT}
            s_endpgm
            """;

        return PixelCase.Uniform("cf_loop_scalar", Program, [7 * 5, 5], 7, 5) with { Raw = true };
    }

    // Per-lane trip count x + 1; lanes retire from EXEC.
    private static PixelCase DivergentLoop()
    {
        const string Program = """
            v_cvt_u32_f32_e32 v10, v2
            v_mov_b32 v20, 0
            s_mov_b32 s12, exec_lo
            .LBB_loop:
            v_add_nc_u32_e32 v20, 1, v20
            v_cmp_gt_u32_e32 vcc_lo, v20, v10
            s_andn2_b32 exec_lo, exec_lo, vcc_lo
            s_cbranch_execnz .LBB_loop
            s_mov_b32 exec_lo, s12
            v_mov_b32 v21, 9
            {EXPORT}
            s_endpgm
            """;

        return new PixelCase("cf_loop_divergent", Program, 2, pixel => [pixel.X + 1, 9]) { Raw = true };
    }

    // EXEC written as an integer, narrowed as a lane mask, then read as an integer and restored: x < 6 runs v21, x < 3
    // runs v20.
    private static PixelCase NestedExecSave()
    {
        const string Program = """
            v_cvt_u32_f32_e32 v10, v2
            v_mov_b32 v20, 0
            v_mov_b32 v21, 0
            s_mov_b32 s13, -1
            s_and_b32 exec_lo, exec_lo, s13
            v_cmp_gt_u32_e32 vcc_lo, 6, v10
            s_and_saveexec_b32 s10, vcc_lo
            s_and_b32 s12, exec_lo, s13
            v_cmp_gt_u32_e32 vcc_lo, 3, v10
            s_and_b32 exec_lo, exec_lo, vcc_lo
            v_mov_b32 v20, 1
            s_mov_b32 exec_lo, s12
            v_mov_b32 v21, 2
            s_or_b32 exec_lo, exec_lo, s10
            {EXPORT}
            s_endpgm
            """;

        return new PixelCase("cf_exec_saved", Program, 2, ExecSavedResult) { Raw = true };
    }

    // EXEC written as an integer, then saved, narrowed and restored as a lane mask: x < 5 runs v20.
    private static PixelCase ExecSaveAfterIntegerWrite()
    {
        const string Program = """
            v_cvt_u32_f32_e32 v10, v2
            v_mov_b32 v20, 0
            s_mov_b32 s13, -1
            s_and_b32 exec_lo, exec_lo, s13
            s_mov_b32 s14, exec_lo
            v_cmp_gt_u32_e32 vcc_lo, 5, v10
            s_and_b32 exec_lo, exec_lo, vcc_lo
            v_mov_b32 v20, 7
            s_mov_b32 exec_lo, s14
            v_add_nc_u32_e32 v21, 1, v20
            {EXPORT}
            s_endpgm
            """;

        return new PixelCase("cf_exec_save_after_integer", Program, 2, ExecSaveAfterIntegerResult) { Raw = true };
    }

    // s_cbranch_vccnz / vccz / s_branch on wave-uniform conditions (the PC wave is one quad; x < 4 is uniform per quad).
    private static PixelCase VccBranches()
    {
        const string Program = """
            v_cvt_u32_f32_e32 v10, v2
            v_mov_b32 v20, 1
            v_cmp_lt_u32_e32 vcc_lo, 0x64, v10
            s_cbranch_vccnz .LBB_skip
            s_branch .LBB_a
            .LBB_skip:
            v_mov_b32 v20, 2
            .LBB_a:
            v_cmp_gt_u32_e32 vcc_lo, 4, v10
            s_cbranch_vccz .LBB_b
            v_mov_b32 v21, 3
            s_branch .LBB_j
            .LBB_b:
            v_mov_b32 v21, 4
            .LBB_j:
            {EXPORT}
            s_endpgm
            """;

        return new PixelCase("cf_vccz", Program, 2, VccResult) { Raw = true };
    }

    // v_cmpx kills half of each quad; the final export (done vm) discards lanes with EXEC clear.
    private static PixelCase DiscardChecker()
    {
        const string Program = """
            v_cvt_u32_f32_e32 v10, v2
            v_cvt_u32_f32_e32 v11, v3
            v_add_nc_u32_e32 v12, v10, v11
            v_and_b32_e32 v12, 1, v12
            v_cmpx_eq_u32_e32 0, v12
            s_cbranch_execnz .LBB_live
            exp null off, off, off, off done vm
            s_endpgm
            .LBB_live:
            v_mov_b32 v20, v10
            v_mov_b32 v21, v11
            {EXPORT}
            s_endpgm
            """;

        return new PixelCase("discard_checker", Program, 2, CheckerResult) { Raw = true };
    }

    // Whole waves killed take the "exp null ... done vm" path.
    private static PixelCase DiscardQuad()
    {
        const string Program = """
            v_cvt_u32_f32_e32 v10, v2
            v_cvt_u32_f32_e32 v11, v3
            v_cmpx_le_u32_e32 4, v10
            s_cbranch_execnz .LBB_live
            exp null off, off, off, off done vm
            s_endpgm
            .LBB_live:
            v_mov_b32 v20, v10
            v_mov_b32 v21, v11
            {EXPORT}
            s_endpgm
            """;

        return new PixelCase("discard_quad", Program, 2, QuadResult) { Raw = true };
    }

    private static Expected[] IfResult(PixelContext pixel)
    {
        if (pixel.X < 3)
        {
            return [1, 100 + pixel.X, 6];
        }

        return [0, 0, 5];
    }

    private static Expected[] IfElseResult(PixelContext pixel)
    {
        if (pixel.X > pixel.Y)
        {
            return [1000 + pixel.X, 1];
        }

        return [2000 + pixel.Y, 1];
    }

    private static Expected[] LongJumpResult(PixelContext pixel)
    {
        if (pixel.X < 3)
        {
            return [10 + pixel.X, 1];
        }

        return [2, 1];
    }

    private static Expected[] ExecSavedResult(PixelContext pixel)
    {
        if (pixel.X < 3)
        {
            return [1, 2];
        }

        if (pixel.X < 6)
        {
            return [0, 2];
        }

        return [0, 0];
    }

    private static Expected[] ExecSaveAfterIntegerResult(PixelContext pixel)
    {
        if (pixel.X < 5)
        {
            return [7, 8];
        }

        return [0, 1];
    }

    private static Expected[] VccResult(PixelContext pixel)
    {
        if (pixel.X < 4)
        {
            return [1, 3];
        }

        return [1, 4];
    }

    /// <summary>Odd x + y pixels are discarded (null).</summary>
    private static Expected[]? CheckerResult(PixelContext pixel)
    {
        if (((pixel.X + pixel.Y) & 1) != 0)
        {
            return null;
        }

        return [pixel.X, pixel.Y];
    }

    /// <summary>Pixels with x &lt; 4 are discarded (null).</summary>
    private static Expected[]? QuadResult(PixelContext pixel)
    {
        if (pixel.X < 4)
        {
            return null;
        }

        return [pixel.X, pixel.Y];
    }
}
