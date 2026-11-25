// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Collections.Frozen;

namespace FiveStar.Shaders.Disassembly.Rdna;

/// <summary>
/// MIMG opcodes with their numbers from the opcode table of the AMD "RDNA 2 Instruction Set Architecture" reference
/// guide (MIMG instructions). The name's suffixes give the address layout (see <see cref="ImageAddress"/>).
/// </summary>
internal static class ImageOpcodes
{
    public static readonly FrozenDictionary<int, string> Table = new Dictionary<int, string>
    {
        [0x00] = "image_load", [0x01] = "image_load_mip", [0x02] = "image_load_pck", [0x03] = "image_load_pck_sgn",
        [0x04] = "image_load_mip_pck", [0x05] = "image_load_mip_pck_sgn", [0x08] = "image_store", [0x09] = "image_store_mip",
        [0x0a] = "image_store_pck", [0x0b] = "image_store_mip_pck", [0x0e] = "image_get_resinfo",
        [0x0f] = "image_atomic_swap", [0x10] = "image_atomic_cmpswap", [0x11] = "image_atomic_add",
        [0x12] = "image_atomic_sub", [0x14] = "image_atomic_smin", [0x15] = "image_atomic_umin",
        [0x16] = "image_atomic_smax", [0x17] = "image_atomic_umax", [0x18] = "image_atomic_and", [0x19] = "image_atomic_or",
        [0x1a] = "image_atomic_xor", [0x1b] = "image_atomic_inc", [0x1c] = "image_atomic_dec",
        [0x1d] = "image_atomic_fcmpswap", [0x1e] = "image_atomic_fmin", [0x1f] = "image_atomic_fmax",
        [0x20] = "image_sample", [0x21] = "image_sample_cl", [0x22] = "image_sample_d", [0x23] = "image_sample_d_cl",
        [0x24] = "image_sample_l", [0x25] = "image_sample_b", [0x26] = "image_sample_b_cl", [0x27] = "image_sample_lz",
        [0x28] = "image_sample_c", [0x29] = "image_sample_c_cl", [0x2a] = "image_sample_c_d", [0x2b] = "image_sample_c_d_cl",
        [0x2c] = "image_sample_c_l", [0x2d] = "image_sample_c_b", [0x2e] = "image_sample_c_b_cl", [0x2f] = "image_sample_c_lz",
        [0x30] = "image_sample_o", [0x31] = "image_sample_cl_o", [0x32] = "image_sample_d_o", [0x33] = "image_sample_d_cl_o",
        [0x34] = "image_sample_l_o", [0x35] = "image_sample_b_o", [0x36] = "image_sample_b_cl_o", [0x37] = "image_sample_lz_o",
        [0x38] = "image_sample_c_o", [0x39] = "image_sample_c_cl_o", [0x3a] = "image_sample_c_d_o",
        [0x3b] = "image_sample_c_d_cl_o", [0x3c] = "image_sample_c_l_o", [0x3d] = "image_sample_c_b_o",
        [0x3e] = "image_sample_c_b_cl_o", [0x3f] = "image_sample_c_lz_o", [0x40] = "image_gather4", [0x41] = "image_gather4_cl",
        [0x44] = "image_gather4_l", [0x45] = "image_gather4_b", [0x46] = "image_gather4_b_cl", [0x47] = "image_gather4_lz",
        [0x48] = "image_gather4_c", [0x49] = "image_gather4_c_cl", [0x4c] = "image_gather4_c_l", [0x4d] = "image_gather4_c_b",
        [0x4e] = "image_gather4_c_b_cl", [0x4f] = "image_gather4_c_lz", [0x50] = "image_gather4_o",
        [0x51] = "image_gather4_cl_o", [0x54] = "image_gather4_l_o", [0x55] = "image_gather4_b_o",
        [0x56] = "image_gather4_b_cl_o", [0x57] = "image_gather4_lz_o", [0x58] = "image_gather4_c_o",
        [0x59] = "image_gather4_c_cl_o", [0x5c] = "image_gather4_c_l_o", [0x5d] = "image_gather4_c_b_o",
        [0x5e] = "image_gather4_c_b_cl_o", [0x5f] = "image_gather4_c_lz_o", [0x60] = "image_get_lod",
        [0x61] = "image_gather4h", [0x68] = "image_sample_cd", [0x69] = "image_sample_cd_cl", [0x6a] = "image_sample_c_cd",
        [0x6b] = "image_sample_c_cd_cl", [0x6c] = "image_sample_cd_o", [0x6d] = "image_sample_cd_cl_o",
        [0x6e] = "image_sample_c_cd_o", [0x6f] = "image_sample_c_cd_cl_o", [0xa2] = "image_sample_d_g16",
        [0xa3] = "image_sample_d_cl_g16", [0xaa] = "image_sample_c_d_g16", [0xab] = "image_sample_c_d_cl_g16",
        [0xb2] = "image_sample_d_o_g16", [0xb3] = "image_sample_d_cl_o_g16", [0xba] = "image_sample_c_d_o_g16",
        [0xbb] = "image_sample_c_d_cl_o_g16", [0xe8] = "image_sample_cd_g16", [0xe9] = "image_sample_cd_cl_g16",
        [0xea] = "image_sample_c_cd_g16", [0xeb] = "image_sample_c_cd_cl_g16", [0xec] = "image_sample_cd_o_g16",
        [0xed] = "image_sample_cd_cl_o_g16", [0xee] = "image_sample_c_cd_o_g16", [0xef] = "image_sample_c_cd_cl_o_g16",
        [0xe6] = "image_bvh_intersect_ray", [0xe7] = "image_bvh64_intersect_ray",
    }.ToFrozenDictionary();
}
