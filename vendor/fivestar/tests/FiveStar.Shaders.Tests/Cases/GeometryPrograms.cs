// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Tests.Cases;

/// <summary>The main program of the handwritten geometry-shader case (see <see cref="GeometryStageRunner"/>).</summary>
public static class GeometryPrograms
{
    /// <summary>
    /// EXEC from the wave's GS thread count, ring base from s_load s[8:9] + 0xb8, input vertex position and texcoord
    /// (ESGS ring: register r component c at dword r * 4 + c) for the 3 vertices, then the two strips. Record layout per
    /// vertex (8 dwords): pos.xy, 0, 1.0, param0 = (vertex number or 100, primitive id, texcoord or (1, 0)).
    /// </summary>
    public const string Main = """
        s_bfe_i32 s0, s3, 0x80008
        s_bfm_b64 exec, s0, 0
        s_cmp_eq_u32 s0, 64
        s_cselect_b64 exec, -1, exec
        s_and_b32 s1, s9, 0xff
        s_mov_b32 s0, s8
        v_mbcnt_lo_u32_b32 v20, -1, 0
        s_load_dword s0, s[0:1], 0xb8
        v_mbcnt_hi_u32_b32 v20, -1, v20
        s_waitcnt lgkmcnt(0)
        s_lshl_b32 s0, s0, 8
        v_mad_u32_u24 v12, 0xe0, v20, s0
        v_and_b32_e32 v3, 0xffff, v0
        v_lshlrev_b32_e32 v3, 2, v3
        v_lshrrev_b32_e32 v4, 16, v0
        v_lshlrev_b32_e32 v4, 2, v4
        v_and_b32_e32 v5, 0xffff, v1
        v_lshlrev_b32_e32 v5, 2, v5
        ds_read2_b32 v[30:31], v3 offset1:1
        ds_read2_b32 v[32:33], v4 offset1:1
        ds_read2_b32 v[34:35], v5 offset1:1
        ds_read2_b32 v[36:37], v3 offset0:4 offset1:5
        ds_read2_b32 v[38:39], v4 offset0:4 offset1:5
        ds_read2_b32 v[44:45], v5 offset0:4 offset1:5
        s_waitcnt lgkmcnt(0)
        v_mov_b32 v40, 0
        v_mov_b32 v41, 1.0
        v_mov_b32 v42, 1
        v_mov_b32 v43, 2
        ds_write2_b32 v12, v30, v31 offset1:1
        ds_write2_b32 v12, v40, v41 offset0:2 offset1:3
        ds_write2_b32 v12, v40, v2 offset0:4 offset1:5
        ds_write2_b32 v12, v36, v37 offset0:6 offset1:7
        s_sendmsg sendmsg(MSG_GS, GS_OP_EMIT, 0)
        ds_write2_b32 v12, v32, v33 offset0:8 offset1:9
        ds_write2_b32 v12, v40, v41 offset0:10 offset1:11
        ds_write2_b32 v12, v42, v2 offset0:12 offset1:13
        ds_write2_b32 v12, v38, v39 offset0:14 offset1:15
        s_sendmsg sendmsg(MSG_GS, GS_OP_EMIT, 0)
        ds_write2_b32 v12, v34, v35 offset0:16 offset1:17
        ds_write2_b32 v12, v40, v41 offset0:18 offset1:19
        ds_write2_b32 v12, v43, v2 offset0:20 offset1:21
        ds_write2_b32 v12, v44, v45 offset0:22 offset1:23
        s_sendmsg sendmsg(MSG_GS, GS_OP_EMIT, 0)
        s_sendmsg sendmsg(MSG_GS, GS_OP_CUT, 0)
        v_cvt_f32_u32_e32 v50, v2
        v_mul_f32_e32 v51, 0.5, v50
        v_sub_f32_e32 v51, 1.0, v51
        v_add_f32_e32 v52, -0.5, v51
        v_mov_b32 v53, 0x64
        ds_write2_b32 v12, v40, v51 offset0:24 offset1:25
        ds_write2_b32 v12, v40, v41 offset0:26 offset1:27
        ds_write2_b32 v12, v53, v2 offset0:28 offset1:29
        ds_write2_b32 v12, v42, v40 offset0:30 offset1:31
        s_sendmsg sendmsg(MSG_GS, GS_OP_EMIT, 0)
        ds_write2_b32 v12, v41, v51 offset0:32 offset1:33
        ds_write2_b32 v12, v40, v41 offset0:34 offset1:35
        ds_write2_b32 v12, v53, v2 offset0:36 offset1:37
        ds_write2_b32 v12, v42, v40 offset0:38 offset1:39
        s_sendmsg sendmsg(MSG_GS, GS_OP_EMIT, 0)
        ds_write2_b32 v12, v40, v52 offset0:40 offset1:41
        ds_write2_b32 v12, v40, v41 offset0:42 offset1:43
        ds_write2_b32 v12, v53, v2 offset0:44 offset1:45
        ds_write2_b32 v12, v42, v40 offset0:46 offset1:47
        s_sendmsg sendmsg(MSG_GS, GS_OP_EMIT, 0)
        ds_write2_b32 v12, v41, v52 offset0:48 offset1:49
        ds_write2_b32 v12, v40, v41 offset0:50 offset1:51
        ds_write2_b32 v12, v53, v2 offset0:52 offset1:53
        ds_write2_b32 v12, v42, v40 offset0:54 offset1:55
        s_sendmsg sendmsg(MSG_GS, GS_OP_EMIT, 0)
        s_sendmsg sendmsg(MSG_GS, GS_OP_CUT, 0)
        s_waitcnt vmcnt(0) expcnt(0) lgkmcnt(0)
        s_sendmsg sendmsg(MSG_GS_DONE, GS_OP_NOP)
        s_endpgm
        """;
}
